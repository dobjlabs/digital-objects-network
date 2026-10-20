use protocol_model::{
    handshake::{
        HandshakeAction, HandshakeState, Message, Orchestrator, OrchestratorRules,
        OrchestratorState, Outcome, Participant, PendingTransaction, ProtocolOrchestrator,
        ProtocolStore, StoreRules, StoreState, StoreStorage, Timer, Token, describe_action,
        end_lease, finish, grant, orchestrator_state, record_pending, save, saved_journal,
        saved_pending, store_state,
    },
    objects::Objects,
    scenarios::{self, Scenario, ScenarioModel, store},
};
use stateright::{
    Checker, Expectation, HasDiscoveries, Model,
    actor::{ActorModelAction, Id, Out, model_timeout},
};

fn replay(
    model: &ScenarioModel,
    actions: impl IntoIterator<Item = HandshakeAction>,
) -> HandshakeState {
    let mut state = model.init_states().remove(0);
    for action in actions {
        let label = describe_action(model, &action);
        state = model
            .next_state(&state, action)
            .unwrap_or_else(|| panic!("disabled step: {label}"));
        for property in model.properties() {
            if property.expectation == Expectation::Always {
                assert!(
                    (property.condition)(model, &state),
                    "{} after {label}",
                    property.name
                );
            }
        }
    }
    state
}

/// The longest counterexample searched for.
const MAX_COUNTEREXAMPLE_STEPS: usize = 40;

/// Searches for a shortest violation of the named safety property, allowing one more
/// step each round, and returns the steps that lead to it.
fn counterexample(model: ScenarioModel, property: &'static str) -> String {
    let safety = model
        .properties()
        .into_iter()
        .find(|candidate| {
            candidate.name == property && candidate.expectation == Expectation::Always
        })
        .unwrap_or_else(|| panic!("no safety property named {property:?}"));
    for depth in 2..=MAX_COUNTEREXAMPLE_STEPS + 1 {
        let checker = model
            .clone()
            .checker()
            .target_max_depth(depth)
            .finish_when(HasDiscoveries::AnyOf([property].into()))
            .spawn_bfs()
            .join();
        let Some(path) = checker.discoveries().remove(property) else {
            continue;
        };
        let mut steps = Vec::new();
        for (state, action) in path.into_vec() {
            if !(safety.condition)(&model, &state) {
                return steps.join(", ");
            }
            steps.extend(action.map(|action| describe_action(&model, &action)));
        }
        panic!("counterexample path never violates {property:?}");
    }
    panic!("no counterexample for {property:?} within {MAX_COUNTEREXAMPLE_STEPS} steps")
}

fn deliver(src: Id, dst: Id, msg: Message) -> HandshakeAction {
    ActorModelAction::Deliver { src, dst, msg }
}

fn timeout(actor: Id, timer: Timer) -> HandshakeAction {
    ActorModelAction::Timeout(actor, timer)
}

fn token(epoch: u8, sequence: u8) -> Token {
    Token { epoch, sequence }
}

/// Every scenario is explored exhaustively within its bounds:
///
/// - Two orchestrators compete for x. Leases expire, renew, and are released, and
///   either orchestrator may give up before the store confirms its transaction is
///   pending.
/// - One orchestrator and the store each crash and restart once, at any point.
/// - Two orchestrators compete for x while the store restarts once, losing every
///   reservation whose transaction was not yet pending.
///
/// Messages may be delayed or reordered without bound throughout, which covers loss.
/// Each scenario also has to reach a submission, so a store that refuses everything
/// would not pass.
#[test]
fn check_all_handshake_interleavings() {
    for scenario in [
        Scenario::Contention,
        Scenario::Recovery,
        Scenario::StoreRestart,
    ] {
        let checker = scenarios::model(scenario).checker().spawn_dfs().join();
        checker.assert_properties();
        println!("{scenario:?}: {} states", checker.unique_state_count());
    }
}

/// Frees a pending transaction's inputs when its former lease expires: marking the
/// transaction pending keeps the lease timer running, and expiry drops it.
struct ExpirePending;

impl StoreRules for ExpirePending {
    fn mark_pending(
        &self,
        state: &mut StoreState,
        from: Id,
        token: Token,
        outputs: Objects,
        o: &mut Out<Participant>,
    ) {
        let newly_pending = state.lease_active(token) && !state.saved.pending.contains_key(&token);
        ProtocolStore.mark_pending(state, from, token, outputs, o);
        if newly_pending {
            o.set_timer(Timer::LeaseExpiry(token), model_timeout());
        }
    }

    fn lease_expired(&self, state: &mut StoreState, token: Token, o: &mut Out<Participant>) {
        if state.saved.pending.remove(&token).is_some() {
            save(state, o);
        }
        ProtocolStore.lease_expired(state, token, o);
    }
}

/// Acknowledges a pending transaction it holds only in memory.
struct AcknowledgeWithoutSaving;

impl StoreRules for AcknowledgeWithoutSaving {
    fn mark_pending(
        &self,
        state: &mut StoreState,
        from: Id,
        token: Token,
        outputs: Objects,
        o: &mut Out<Participant>,
    ) {
        if state.saved.pending.contains_key(&token) || !state.lease_active(token) {
            return ProtocolStore.mark_pending(state, from, token, outputs, o);
        }
        let transaction = PendingTransaction {
            inputs: state.leases[&token].inputs,
            outputs,
        };
        end_lease(state, token, o);
        state.saved.pending.insert(token, transaction);
        o.send(from, Message::MarkedPending(token));
    }
}

/// Marks a transaction pending under an ended lease while another lease is active.
struct AcceptStaleReservation;

impl StoreRules for AcceptStaleReservation {
    fn mark_pending(
        &self,
        state: &mut StoreState,
        from: Id,
        token: Token,
        outputs: Objects,
        o: &mut Out<Participant>,
    ) {
        let ended = state.leases.get(&token).is_some_and(|lease| !lease.active);
        let another_active = state.leases.values().any(|lease| lease.active);
        if ended && another_active && !state.saved.pending.contains_key(&token) {
            record_pending(state, from, token, outputs, o);
        } else {
            ProtocolStore.mark_pending(state, from, token, outputs, o);
        }
    }
}

/// Revives an ended lease when asked to renew it.
struct RenewExpiredLease;

impl StoreRules for RenewExpiredLease {
    fn renew(&self, state: &mut StoreState, token: Token, o: &mut Out<Participant>) {
        if !state.saved.pending.contains_key(&token)
            && let Some(lease) = state.leases.get_mut(&token)
        {
            lease.active = true;
            o.set_timer(Timer::LeaseExpiry(token), model_timeout());
        }
    }
}

/// Honors a release of a reservation whose transaction is pending.
struct ReleasePending;

impl StoreRules for ReleasePending {
    fn release(&self, state: &mut StoreState, from: Id, token: Token, o: &mut Out<Participant>) {
        if state.saved.pending.remove(&token).is_some() {
            save(state, o);
            o.send(from, Message::Released(token));
        } else {
            ProtocolStore.release(state, from, token, o);
        }
    }
}

/// Reserves inputs without checking pending transactions.
struct ReserveOverPendingInputs;

impl StoreRules for ReserveOverPendingInputs {
    fn reserve(&self, state: &mut StoreState, from: Id, inputs: Objects, o: &mut Out<Participant>) {
        if state.leases_cover(inputs) {
            o.send(from, Message::Refused);
        } else {
            grant(state, from, inputs, o);
        }
    }
}

/// Saves a pending transaction without the objects it creates.
struct SaveWithoutOutputs;

impl StoreRules for SaveWithoutOutputs {
    fn mark_pending(
        &self,
        state: &mut StoreState,
        from: Id,
        token: Token,
        outputs: Objects,
        o: &mut Out<Participant>,
    ) {
        if state.saved.pending.contains_key(&token) || !state.lease_active(token) {
            return ProtocolStore.mark_pending(state, from, token, outputs, o);
        }
        record_pending(state, from, token, Objects::default(), o);
    }
}

/// Loses the saved pending transactions when it restarts.
struct ForgetPendingOnRestart;

impl StoreRules for ForgetPendingOnRestart {
    fn restart(&self, previous: Option<&StoreStorage>) -> StoreStorage {
        let mut storage = ProtocolStore.restart(previous);
        storage.pending.clear();
        storage
    }
}

/// Keeps its token epoch across a restart, so it issues the tokens from before the
/// restart again.
struct ReissueTokens;

impl StoreRules for ReissueTokens {
    fn restart(&self, previous: Option<&StoreStorage>) -> StoreStorage {
        previous.cloned().unwrap_or_default()
    }
}

/// Answers a release but keeps the lease.
struct IgnoreRelease;

impl StoreRules for IgnoreRelease {
    fn release(&self, state: &mut StoreState, from: Id, token: Token, o: &mut Out<Participant>) {
        if state.saved.pending.contains_key(&token) {
            o.send(from, Message::StillPending(token));
        } else {
            o.send(from, Message::Released(token));
        }
    }
}

/// Submits as soon as it asks for its transaction to be marked pending, without
/// waiting for the acknowledgement.
struct SubmitWithoutAcknowledgement;

impl OrchestratorRules for SubmitWithoutAcknowledgement {
    fn proof_ready(
        &self,
        orchestrator: &Orchestrator,
        state: &mut OrchestratorState,
        token: Token,
        o: &mut Out<Participant>,
    ) {
        ProtocolOrchestrator.proof_ready(orchestrator, state, token, o);
        finish(state, Outcome::Submitted, o);
    }
}

/// One participant at a time breaks a rule while the others follow the protocol.
/// For each, the checker must find a run violating the guarantee the rule protects:
///
/// - Expiring, releasing, or overlooking a pending transaction lets a second
///   orchestrator be acknowledged for the same input, as does accepting or reviving
///   an ended lease.
/// - Acknowledging without saving, or losing saved pending transactions on restart,
///   leaves an acknowledgement with nothing durable behind it.
/// - Saving a transaction without the objects it creates leaves a saved copy that is
///   not what the orchestrator asked for.
/// - Issuing the tokens from before a restart again gives two reservations one token.
/// - Keeping a lease after answering its release lets an abandoned transaction be
///   acknowledged.
/// - Submitting without waiting for the acknowledgement submits a transaction the
///   store may never have saved as pending.
#[test]
fn handshake_faults_have_specific_counterexamples() {
    let exclusive = "acknowledged transactions are exclusive";
    let saved = "acknowledgements are saved";
    let faults: [(
        &str,
        Scenario,
        &'static dyn StoreRules,
        &'static dyn OrchestratorRules,
        &'static str,
    ); 11] = [
        (
            "ExpirePending",
            Scenario::Contention,
            &ExpirePending,
            &ProtocolOrchestrator,
            exclusive,
        ),
        (
            "AcceptStaleReservation",
            Scenario::Contention,
            &AcceptStaleReservation,
            &ProtocolOrchestrator,
            exclusive,
        ),
        (
            "RenewExpiredLease",
            Scenario::Contention,
            &RenewExpiredLease,
            &ProtocolOrchestrator,
            exclusive,
        ),
        (
            "ReleasePending",
            Scenario::Contention,
            &ReleasePending,
            &ProtocolOrchestrator,
            exclusive,
        ),
        (
            "ReserveOverPendingInputs",
            Scenario::Contention,
            &ReserveOverPendingInputs,
            &ProtocolOrchestrator,
            exclusive,
        ),
        (
            "AcknowledgeWithoutSaving",
            Scenario::Contention,
            &AcknowledgeWithoutSaving,
            &ProtocolOrchestrator,
            saved,
        ),
        (
            "ForgetPendingOnRestart",
            Scenario::Recovery,
            &ForgetPendingOnRestart,
            &ProtocolOrchestrator,
            saved,
        ),
        (
            "SaveWithoutOutputs",
            Scenario::Contention,
            &SaveWithoutOutputs,
            &ProtocolOrchestrator,
            "saved transactions match their requests",
        ),
        (
            "ReissueTokens",
            Scenario::StoreRestart,
            &ReissueTokens,
            &ProtocolOrchestrator,
            "tokens are issued once",
        ),
        (
            "IgnoreRelease",
            Scenario::Contention,
            &IgnoreRelease,
            &ProtocolOrchestrator,
            "released tokens stay unacknowledged",
        ),
        (
            "SubmitWithoutAcknowledgement",
            Scenario::Recovery,
            &ProtocolStore,
            &SubmitWithoutAcknowledgement,
            "submissions are saved as pending",
        ),
    ];
    for (name, scenario, store_rules, orchestrator_rules, property) in faults {
        let model = scenarios::with_rules(scenario, store_rules, orchestrator_rules);
        println!("{name}: {}", counterexample(model, property));
    }
}

/// The first orchestrator reserves x and y, proves, saves its payload, and asks the
/// store to mark its transaction pending. Before the request arrives, its lease
/// ends: either the lease expires, or the orchestrator gives up and releases it. The
/// second orchestrator then reserves x and z.
///
/// When the delayed request finally arrives, the store must reject it. The token's
/// lease has ended, and x now belongs to another reservation; accepting the request
/// would authorize two transactions to consume x.
#[test]
fn delayed_mark_pending_cannot_use_an_ended_lease() {
    let model = scenarios::model(Scenario::Contention);
    let [first, second] = [Id::from(1), Id::from(2)];
    let first_token = token(0, 0);
    let mark_pending = Message::MarkPending {
        token: first_token,
        outputs: Objects::P,
    };
    let reserved_and_proved = [
        deliver(
            first,
            store(),
            Message::Reserve(Objects::X.union(Objects::Y)),
        ),
        deliver(store(), first, Message::Reserved(first_token)),
        timeout(first, Timer::ProofReady),
    ];
    for lease_ends in [
        vec![timeout(store(), Timer::LeaseExpiry(first_token))],
        vec![
            timeout(first, Timer::GiveUp),
            deliver(first, store(), Message::Release(first_token)),
        ],
    ] {
        let state = replay(
            &model,
            reserved_and_proved
                .clone()
                .into_iter()
                .chain(lease_ends)
                .chain([
                    deliver(
                        second,
                        store(),
                        Message::Reserve(Objects::X.union(Objects::Z)),
                    ),
                    deliver(first, store(), mark_pending.clone()),
                ]),
        );
        assert!(!saved_pending(&state, store()).contains_key(&first_token));
        assert!(store_state(&state, store()).leases[&token(0, 1)].active);
    }
}

/// The store saves the orchestrator's transaction as pending, but the
/// acknowledgement never reaches the orchestrator. The store then crashes and
/// restarts, followed by the orchestrator.
///
/// The restarted orchestrator does not know whether its request succeeded, so it
/// repeats the request from its journal. The store finds the pending transaction it
/// saved before the restart and acknowledges it again, and the orchestrator
/// submits. The restart lost every lease, but not the pending transaction.
#[test]
fn lost_acknowledgement_is_recovered_after_both_restarts() {
    let model = scenarios::model(Scenario::Recovery);
    let orchestrator = Id::from(1);
    let reservation = token(0, 0);
    let mark_pending = Message::MarkPending {
        token: reservation,
        outputs: Objects::P,
    };
    let state = replay(
        &model,
        [
            deliver(
                orchestrator,
                store(),
                Message::Reserve(Objects::X.union(Objects::Y)),
            ),
            deliver(store(), orchestrator, Message::Reserved(reservation)),
            timeout(orchestrator, Timer::ProofReady),
            deliver(orchestrator, store(), mark_pending.clone()),
            // The transaction is saved as pending; its acknowledgement is not delivered.
            ActorModelAction::Crash(store()),
            ActorModelAction::Recover(store()),
            ActorModelAction::Crash(orchestrator),
            ActorModelAction::Recover(orchestrator),
            deliver(orchestrator, store(), mark_pending),
            deliver(store(), orchestrator, Message::MarkedPending(reservation)),
        ],
    );
    assert_eq!(
        saved_journal(&state, orchestrator).outcome,
        Some(Outcome::Submitted)
    );
    assert!(orchestrator_state(&state, orchestrator).restarted);
    assert!(saved_pending(&state, store()).contains_key(&reservation));
    assert!(store_state(&state, store()).leases.is_empty());
}

/// The first orchestrator holds a lease on x and y and has asked the store to mark
/// its transaction pending when the store restarts. Reservations are not saved, so
/// the lease is gone, and the second orchestrator reserves x and z under a token
/// from the new epoch.
///
/// The first orchestrator's request then arrives with its old token and is
/// rejected. It reserves again, but x is taken, so it gives up. The second
/// orchestrator's transaction is marked pending and submitted. Forgetting the
/// reservation cost the first orchestrator its transaction, but never let both
/// transactions hold x.
#[test]
fn store_restart_ends_reservations_without_sharing_inputs() {
    let model = scenarios::model(Scenario::StoreRestart);
    let [first, second] = [Id::from(1), Id::from(2)];
    let first_token = token(0, 0);
    let second_token = token(1, 0);
    let state = replay(
        &model,
        [
            deliver(
                first,
                store(),
                Message::Reserve(Objects::X.union(Objects::Y)),
            ),
            deliver(store(), first, Message::Reserved(first_token)),
            timeout(first, Timer::ProofReady),
            ActorModelAction::Crash(store()),
            ActorModelAction::Recover(store()),
            deliver(
                second,
                store(),
                Message::Reserve(Objects::X.union(Objects::Z)),
            ),
            deliver(
                first,
                store(),
                Message::MarkPending {
                    token: first_token,
                    outputs: Objects::P,
                },
            ),
            deliver(store(), first, Message::NotReserved(first_token)),
            deliver(
                first,
                store(),
                Message::Reserve(Objects::X.union(Objects::Y)),
            ),
            deliver(store(), first, Message::Refused),
            deliver(store(), second, Message::Reserved(second_token)),
            timeout(second, Timer::ProofReady),
            deliver(
                second,
                store(),
                Message::MarkPending {
                    token: second_token,
                    outputs: Objects::Q,
                },
            ),
            deliver(store(), second, Message::MarkedPending(second_token)),
        ],
    );
    assert_eq!(
        saved_journal(&state, first).outcome,
        Some(Outcome::Abandoned)
    );
    assert_eq!(
        saved_journal(&state, second).outcome,
        Some(Outcome::Submitted)
    );
    assert_eq!(
        saved_pending(&state, store()).keys().collect::<Vec<_>>(),
        [&second_token]
    );
}
