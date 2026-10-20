use protocol_model::{
    handshake::{
        HandshakeAction, HandshakeState, Message, Orchestrator, OrchestratorRules,
        OrchestratorState, Outcome, Participant, PendingTransaction, ProtocolOrchestrator,
        ProtocolStore, Reservation, Store, StoreRules, StoreState, StoreStorage, Timer, Verdict,
        describe_action, end_lease, finish, grant, orchestrator_state, record_pending, save,
        saved_journal, saved_pending, store_state,
    },
    objects::Objects,
    scenarios::{self, Scenario, ScenarioModel, store, synchronizer},
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
/// step each round, and returns the steps that lead to it. One search per depth is
/// what makes the result shortest: a single search stops only at the end of a block
/// of states and keeps the last violation it saw.
fn counterexample(model: ScenarioModel, property: &'static str) -> String {
    let safety = Model::property(&model, property);
    assert_eq!(
        safety.expectation,
        Expectation::Always,
        "{property:?} is not a safety property"
    );
    for depth in 2..=MAX_COUNTEREXAMPLE_STEPS + 1 {
        let checker = model
            .clone()
            .checker()
            .target_max_depth(depth)
            .finish_when(HasDiscoveries::AnyOf([property].into()))
            .spawn_bfs()
            .join();
        let Some(path) = checker.discovery(property) else {
            continue;
        };
        assert!(
            !(safety.condition)(&model, path.last_state()),
            "the path found for {property:?} ends in a state that satisfies it"
        );
        return path
            .into_actions()
            .iter()
            .map(|action| describe_action(&model, action))
            .collect::<Vec<_>>()
            .join(", ");
    }
    panic!("no counterexample for {property:?} within {MAX_COUNTEREXAMPLE_STEPS} steps")
}

fn deliver(src: Id, dst: Id, msg: Message) -> HandshakeAction {
    ActorModelAction::Deliver { src, dst, msg }
}

fn timeout(actor: Id, timer: Timer) -> HandshakeAction {
    ActorModelAction::Timeout(actor, timer)
}

fn granted(epoch: u8, sequence: u8) -> Reservation {
    Reservation { epoch, sequence }
}

fn mark_pending(orchestrator: Id, reservation: Reservation, outputs: Objects) -> HandshakeAction {
    deliver(
        orchestrator,
        store(),
        Message::MarkPending {
            reservation,
            outputs,
        },
    )
}

/// The orchestrator's reservation request reaches the store, the grant reaches the
/// orchestrator, and its proof finishes.
fn reserve_and_prove(
    orchestrator: Id,
    inputs: Objects,
    reservation: Reservation,
) -> [HandshakeAction; 3] {
    [
        deliver(orchestrator, store(), Message::Reserve(inputs)),
        deliver(store(), orchestrator, Message::Reserved(reservation)),
        timeout(orchestrator, Timer::ProofReady),
    ]
}

/// The store's watch request reaches the synchronizer, which settles the transaction
/// with the verdict, and the settlement reaches the store.
fn settle(
    scenario: Scenario,
    reservation: Reservation,
    inputs: Objects,
    outputs: Objects,
    verdict: Verdict,
) -> [HandshakeAction; 3] {
    let synchronizer = synchronizer(scenario);
    [
        deliver(
            store(),
            synchronizer,
            Message::Watch {
                reservation,
                inputs,
                outputs,
            },
        ),
        timeout(synchronizer, Timer::Settle(reservation, verdict)),
        deliver(
            synchronizer,
            store(),
            Message::Settled {
                reservation,
                verdict,
            },
        ),
    ]
}

/// Whether the protocol's store would save a new pending transaction for a request
/// under this reservation.
fn protocol_would_record(state: &StoreState, reservation: Reservation) -> bool {
    state.lease_active(reservation) && !state.saved.pending.contains_key(&reservation)
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
        store: &Store,
        state: &mut StoreState,
        from: Id,
        reservation: Reservation,
        outputs: Objects,
        o: &mut Out<Participant>,
    ) {
        let newly_pending = protocol_would_record(state, reservation);
        ProtocolStore.mark_pending(store, state, from, reservation, outputs, o);
        if newly_pending {
            o.set_timer(Timer::LeaseExpiry(reservation), model_timeout());
        }
    }

    fn lease_expired(
        &self,
        store: &Store,
        state: &mut StoreState,
        reservation: Reservation,
        o: &mut Out<Participant>,
    ) {
        if state.saved.pending.remove(&reservation).is_some() {
            save(&state.saved, o);
        }
        ProtocolStore.lease_expired(store, state, reservation, o);
    }
}

/// Acknowledges a pending transaction it holds only in memory.
struct AcknowledgeWithoutSaving;

impl StoreRules for AcknowledgeWithoutSaving {
    fn mark_pending(
        &self,
        store: &Store,
        state: &mut StoreState,
        from: Id,
        reservation: Reservation,
        outputs: Objects,
        o: &mut Out<Participant>,
    ) {
        if !protocol_would_record(state, reservation) {
            return ProtocolStore.mark_pending(store, state, from, reservation, outputs, o);
        }
        let transaction = PendingTransaction {
            inputs: state.leases[&reservation].inputs,
            outputs,
            verdict: None,
        };
        end_lease(state, reservation, o);
        state.saved.pending.insert(reservation, transaction);
        o.send(from, Message::MarkedPending(reservation));
    }
}

/// Marks a transaction pending under an ended lease while another lease is active.
struct AcceptStaleReservation;

impl StoreRules for AcceptStaleReservation {
    fn mark_pending(
        &self,
        store: &Store,
        state: &mut StoreState,
        from: Id,
        reservation: Reservation,
        outputs: Objects,
        o: &mut Out<Participant>,
    ) {
        let ended = state
            .leases
            .get(&reservation)
            .is_some_and(|lease| !lease.active);
        let another_active = state.leases.values().any(|lease| lease.active);
        if ended && another_active && !state.saved.pending.contains_key(&reservation) {
            record_pending(store, state, from, reservation, outputs, o);
        } else {
            ProtocolStore.mark_pending(store, state, from, reservation, outputs, o);
        }
    }
}

/// Revives an ended lease when asked to renew it.
struct RenewExpiredLease;

impl StoreRules for RenewExpiredLease {
    fn renew(
        &self,
        _: &Store,
        state: &mut StoreState,
        reservation: Reservation,
        o: &mut Out<Participant>,
    ) {
        if !state.saved.pending.contains_key(&reservation)
            && let Some(lease) = state.leases.get_mut(&reservation)
        {
            lease.active = true;
            o.set_timer(Timer::LeaseExpiry(reservation), model_timeout());
        }
    }
}

/// Honors a release of a reservation whose transaction is pending.
struct ReleasePending;

impl StoreRules for ReleasePending {
    fn release(
        &self,
        store: &Store,
        state: &mut StoreState,
        from: Id,
        reservation: Reservation,
        o: &mut Out<Participant>,
    ) {
        if state.saved.pending.remove(&reservation).is_some() {
            save(&state.saved, o);
            o.send(from, Message::Released(reservation));
        } else {
            ProtocolStore.release(store, state, from, reservation, o);
        }
    }
}

/// Reserves inputs without checking pending transactions.
struct ReserveOverPendingInputs;

impl StoreRules for ReserveOverPendingInputs {
    fn reserve(
        &self,
        _: &Store,
        state: &mut StoreState,
        from: Id,
        inputs: Objects,
        o: &mut Out<Participant>,
    ) {
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
        store: &Store,
        state: &mut StoreState,
        from: Id,
        reservation: Reservation,
        outputs: Objects,
        o: &mut Out<Participant>,
    ) {
        if !protocol_would_record(state, reservation) {
            return ProtocolStore.mark_pending(store, state, from, reservation, outputs, o);
        }
        record_pending(store, state, from, reservation, Objects::default(), o);
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

/// Keeps its epoch across a restart, so it grants the reservations from before the
/// restart again.
struct RegrantReservations;

impl StoreRules for RegrantReservations {
    fn restart(&self, previous: Option<&StoreStorage>) -> StoreStorage {
        previous.cloned().unwrap_or_default()
    }
}

/// Reserves inputs that a settlement consumed.
struct ReserveConsumedInputs;

impl StoreRules for ReserveConsumedInputs {
    fn reserve(
        &self,
        _: &Store,
        state: &mut StoreState,
        from: Id,
        inputs: Objects,
        o: &mut Out<Participant>,
    ) {
        if state.leases_cover(inputs) || state.pending_covers(inputs) {
            o.send(from, Message::Refused);
        } else {
            grant(state, from, inputs, o);
        }
    }
}

/// Acknowledges a repeated request for a transaction that has been settled.
struct AcknowledgeAfterSettlement;

impl StoreRules for AcknowledgeAfterSettlement {
    fn mark_pending(
        &self,
        store: &Store,
        state: &mut StoreState,
        from: Id,
        reservation: Reservation,
        outputs: Objects,
        o: &mut Out<Participant>,
    ) {
        if state
            .saved
            .pending
            .get(&reservation)
            .is_some_and(PendingTransaction::ended)
        {
            o.send(from, Message::MarkedPending(reservation));
        } else {
            ProtocolStore.mark_pending(store, state, from, reservation, outputs, o);
        }
    }
}

/// Applies a settlement in memory without saving it.
struct SettleWithoutSaving;

impl StoreRules for SettleWithoutSaving {
    fn settled(
        &self,
        _: &Store,
        state: &mut StoreState,
        reservation: Reservation,
        verdict: Verdict,
        _: &mut Out<Participant>,
    ) {
        if let Some(transaction) = state.saved.pending.get_mut(&reservation)
            && !transaction.ended()
        {
            transaction.verdict = Some(verdict);
        }
    }
}

/// Loses the saved settlements when it restarts.
struct ForgetSettlementOnRestart;

impl StoreRules for ForgetSettlementOnRestart {
    fn restart(&self, previous: Option<&StoreStorage>) -> StoreStorage {
        let mut storage = ProtocolStore.restart(previous);
        for transaction in storage.pending.values_mut() {
            transaction.verdict = None;
        }
        storage
    }
}

/// Answers a release but keeps the lease.
struct IgnoreRelease;

impl StoreRules for IgnoreRelease {
    fn release(
        &self,
        _: &Store,
        state: &mut StoreState,
        from: Id,
        reservation: Reservation,
        o: &mut Out<Participant>,
    ) {
        if state.saved.pending.contains_key(&reservation) {
            o.send(from, Message::StillPending(reservation));
        } else {
            o.send(from, Message::Released(reservation));
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
        reservation: Reservation,
        o: &mut Out<Participant>,
    ) {
        ProtocolOrchestrator.proof_ready(orchestrator, state, reservation, o);
        finish(state, Outcome::Submitted, o);
    }
}

/// One participant with a rule replaced, and the guarantee that rule protects.
struct Fault {
    name: &'static str,
    scenario: Scenario,
    store: &'static dyn StoreRules,
    orchestrator: &'static dyn OrchestratorRules,
    violates: &'static str,
}

/// The fault's type name, so that a rename cannot leave a stale label.
fn type_name_of<T>(_: &T) -> &'static str {
    std::any::type_name::<T>()
        .rsplit("::")
        .next()
        .expect("type names are not empty")
}

fn store_fault<R: StoreRules>(
    scenario: Scenario,
    rules: &'static R,
    violates: &'static str,
) -> Fault {
    Fault {
        name: type_name_of(rules),
        scenario,
        store: rules,
        orchestrator: &ProtocolOrchestrator,
        violates,
    }
}

fn orchestrator_fault<R: OrchestratorRules>(
    scenario: Scenario,
    rules: &'static R,
    violates: &'static str,
) -> Fault {
    Fault {
        name: type_name_of(rules),
        scenario,
        store: &ProtocolStore,
        orchestrator: rules,
        violates,
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
/// - Granting the reservations from before a restart again gives two orchestrators
///   the same reservation.
/// - Reserving an input that a settlement consumed lets a second transaction be
///   acknowledged for it.
/// - Applying a settlement without saving it, or losing saved settlements on
///   restart, leaves the store's record behind what the synchronizer said.
/// - Acknowledging a settled transaction again sends its orchestrator to submit a
///   payload the chain no longer accepts.
/// - Keeping a lease after answering its release lets an abandoned transaction be
///   acknowledged.
/// - Submitting without waiting for the acknowledgement submits a transaction the
///   store may never have saved as pending.
#[test]
fn handshake_faults_have_specific_counterexamples() {
    let exclusive = "pending transactions are exclusive";
    let saved = "acknowledgements are saved";
    let settlements = "settlements are saved";
    let faults = [
        store_fault(Scenario::Contention, &ExpirePending, exclusive),
        store_fault(Scenario::Contention, &AcceptStaleReservation, exclusive),
        store_fault(Scenario::Contention, &RenewExpiredLease, exclusive),
        store_fault(Scenario::Contention, &ReleasePending, exclusive),
        store_fault(Scenario::Contention, &ReserveOverPendingInputs, exclusive),
        store_fault(Scenario::Contention, &AcknowledgeWithoutSaving, saved),
        store_fault(Scenario::Recovery, &ForgetPendingOnRestart, saved),
        store_fault(
            Scenario::Contention,
            &SaveWithoutOutputs,
            "saved transactions match their requests",
        ),
        store_fault(
            Scenario::StoreRestart,
            &RegrantReservations,
            "reservations are granted once",
        ),
        store_fault(Scenario::Contention, &ReserveConsumedInputs, exclusive),
        store_fault(Scenario::Contention, &SettleWithoutSaving, settlements),
        store_fault(Scenario::Recovery, &ForgetSettlementOnRestart, settlements),
        store_fault(
            Scenario::Recovery,
            &AcknowledgeAfterSettlement,
            "settled transactions are not acknowledged",
        ),
        store_fault(
            Scenario::Contention,
            &IgnoreRelease,
            "released reservations stay unacknowledged",
        ),
        orchestrator_fault(
            Scenario::Recovery,
            &SubmitWithoutAcknowledgement,
            "submissions are saved",
        ),
    ];
    for fault in faults {
        let model = scenarios::with_rules(fault.scenario, fault.store, fault.orchestrator);
        println!("{}: {}", fault.name, counterexample(model, fault.violates));
    }
}

/// The first orchestrator reserves x and y, proves, saves its payload, and asks the
/// store to mark its transaction pending. Before the request arrives, its lease
/// ends: either the lease expires, or the orchestrator gives up and releases it. The
/// second orchestrator then reserves x and z.
///
/// When the delayed request finally arrives, the store must reject it. The reservation's
/// lease has ended, and x now belongs to another reservation; accepting the request
/// would authorize two transactions to consume x.
#[test]
fn delayed_mark_pending_cannot_use_an_ended_lease() {
    let model = scenarios::model(Scenario::Contention);
    let [first, second] = [Id::from(1), Id::from(2)];
    let first_reservation = granted(0, 0);
    for lease_ends in [
        vec![timeout(store(), Timer::LeaseExpiry(first_reservation))],
        vec![
            timeout(first, Timer::GiveUp),
            deliver(first, store(), Message::Release(first_reservation)),
        ],
    ] {
        let state = replay(
            &model,
            reserve_and_prove(first, Objects::X.union(Objects::Y), first_reservation)
                .into_iter()
                .chain(lease_ends)
                .chain([
                    deliver(
                        second,
                        store(),
                        Message::Reserve(Objects::X.union(Objects::Z)),
                    ),
                    mark_pending(first, first_reservation, Objects::P),
                ]),
        );
        assert!(!saved_pending(&state, store()).contains_key(&first_reservation));
        assert!(store_state(&state, store()).leases[&granted(0, 1)].active);
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
    let reservation = granted(0, 0);
    let state = replay(
        &model,
        reserve_and_prove(orchestrator, Objects::X.union(Objects::Y), reservation)
            .into_iter()
            .chain([
                mark_pending(orchestrator, reservation, Objects::P),
                // The transaction is saved as pending; its acknowledgement is not delivered.
                ActorModelAction::Crash(store()),
                ActorModelAction::Recover(store()),
                ActorModelAction::Crash(orchestrator),
                ActorModelAction::Recover(orchestrator),
                mark_pending(orchestrator, reservation, Objects::P),
                deliver(store(), orchestrator, Message::MarkedPending(reservation)),
            ]),
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
/// its transaction pending when the store restarts. Leases are not saved, so
/// the lease is gone, and the second orchestrator reserves x and z under a reservation
/// from the new epoch.
///
/// The first orchestrator's request then arrives with its old reservation and is
/// rejected. It reserves again, but x is taken, so it gives up. The second
/// orchestrator's transaction is marked pending and submitted. Forgetting the
/// reservation cost the first orchestrator its transaction, but never let both
/// transactions hold x.
#[test]
fn store_restart_ends_reservations_without_sharing_inputs() {
    let model = scenarios::model(Scenario::StoreRestart);
    let [first, second] = [Id::from(1), Id::from(2)];
    let first_reservation = granted(0, 0);
    let second_reservation = granted(1, 0);
    let state = replay(
        &model,
        reserve_and_prove(first, Objects::X.union(Objects::Y), first_reservation)
            .into_iter()
            .chain([
                ActorModelAction::Crash(store()),
                ActorModelAction::Recover(store()),
                deliver(
                    second,
                    store(),
                    Message::Reserve(Objects::X.union(Objects::Z)),
                ),
                mark_pending(first, first_reservation, Objects::P),
                deliver(store(), first, Message::NotReserved(first_reservation)),
                deliver(
                    first,
                    store(),
                    Message::Reserve(Objects::X.union(Objects::Y)),
                ),
                deliver(store(), first, Message::Refused),
                deliver(store(), second, Message::Reserved(second_reservation)),
                timeout(second, Timer::ProofReady),
                mark_pending(second, second_reservation, Objects::Q),
                deliver(store(), second, Message::MarkedPending(second_reservation)),
            ]),
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
        [&second_reservation]
    );
}

/// The first orchestrator's transaction is marked pending, but before the
/// acknowledgement reaches it the synchronizer settles the transaction as timed out:
/// nothing of it appeared on chain. The late acknowledgement still makes the
/// orchestrator submit, a payload the chain will not accept, which is harmless
/// because the settlement is exact. The settlement freed x, so the second
/// orchestrator reserves it.
#[test]
fn timed_out_transaction_frees_its_inputs_despite_a_late_acknowledgement() {
    let scenario = Scenario::Contention;
    let model = scenarios::model(scenario);
    let [first, second] = [Id::from(1), Id::from(2)];
    let first_reservation = granted(0, 0);
    let inputs = Objects::X.union(Objects::Y);
    let timed_out = Verdict::default();
    let state = replay(
        &model,
        reserve_and_prove(first, inputs, first_reservation)
            .into_iter()
            .chain([mark_pending(first, first_reservation, Objects::P)])
            .chain(settle(
                scenario,
                first_reservation,
                inputs,
                Objects::P,
                timed_out,
            ))
            .chain([
                deliver(store(), first, Message::MarkedPending(first_reservation)),
                deliver(
                    second,
                    store(),
                    Message::Reserve(Objects::X.union(Objects::Z)),
                ),
                deliver(store(), second, Message::Reserved(granted(0, 1))),
            ]),
    );
    assert_eq!(
        saved_journal(&state, first).outcome,
        Some(Outcome::Submitted)
    );
    assert_eq!(
        saved_pending(&state, store())[&first_reservation].verdict,
        Some(timed_out)
    );
    assert!(store_state(&state, store()).leases[&granted(0, 1)].active);
}

/// The first orchestrator's transaction lands: every input consumed and every output
/// created. Its inputs are nullified, so the second orchestrator's reservation of x
/// is refused even though no lease or pending transaction holds it any more.
#[test]
fn landed_transaction_keeps_its_inputs_consumed() {
    let scenario = Scenario::Contention;
    let model = scenarios::model(scenario);
    let [first, second] = [Id::from(1), Id::from(2)];
    let first_reservation = granted(0, 0);
    let inputs = Objects::X.union(Objects::Y);
    let landed = Verdict {
        consumed: inputs,
        created: Objects::P,
    };
    let state = replay(
        &model,
        reserve_and_prove(first, inputs, first_reservation)
            .into_iter()
            .chain([
                mark_pending(first, first_reservation, Objects::P),
                deliver(store(), first, Message::MarkedPending(first_reservation)),
            ])
            .chain(settle(
                scenario,
                first_reservation,
                inputs,
                Objects::P,
                landed,
            ))
            .chain([
                deliver(
                    second,
                    store(),
                    Message::Reserve(Objects::X.union(Objects::Z)),
                ),
                deliver(store(), second, Message::Refused),
            ]),
    );
    assert!(saved_pending(&state, store())[&first_reservation].landed());
    assert_eq!(store_state(&state, store()).nullified(), inputs);
    assert_eq!(
        saved_journal(&state, second).outcome,
        Some(Outcome::Abandoned)
    );
}

/// The store saves the orchestrator's transaction as pending and the synchronizer
/// settles it: another transaction consumed x, so this one can never land. The
/// acknowledgement is lost, and both the store and the orchestrator restart. The
/// repeated request finds the settlement, which survived the restart, and the
/// orchestrator learns the transaction ended instead of submitting it.
#[test]
fn settlement_survives_a_restart_and_ends_the_repeated_request() {
    let scenario = Scenario::Recovery;
    let model = scenarios::model(scenario);
    let orchestrator = Id::from(1);
    let reservation = granted(0, 0);
    let inputs = Objects::X.union(Objects::Y);
    let impossible = Verdict {
        consumed: Objects::X,
        created: Objects::default(),
    };
    let state = replay(
        &model,
        reserve_and_prove(orchestrator, inputs, reservation)
            .into_iter()
            .chain([mark_pending(orchestrator, reservation, Objects::P)])
            .chain(settle(
                scenario,
                reservation,
                inputs,
                Objects::P,
                impossible,
            ))
            .chain([
                ActorModelAction::Crash(store()),
                ActorModelAction::Recover(store()),
                ActorModelAction::Crash(orchestrator),
                ActorModelAction::Recover(orchestrator),
                mark_pending(orchestrator, reservation, Objects::P),
                deliver(store(), orchestrator, Message::Ended(reservation)),
            ]),
    );
    assert_eq!(
        saved_journal(&state, orchestrator).outcome,
        Some(Outcome::Abandoned)
    );
    assert_eq!(
        saved_pending(&state, store())[&reservation].verdict,
        Some(impossible)
    );
    assert_eq!(store_state(&state, store()).nullified(), Objects::X);
}
