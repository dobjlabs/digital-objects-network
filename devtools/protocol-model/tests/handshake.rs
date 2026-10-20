use protocol_model::{
    handshake::{
        self, Fault, Message, Outcome, Scenario, Timer, Token, orchestrator_state, saved_journal,
        saved_preparations, store, store_state,
    },
    objects::Objects,
};
use stateright::{
    Checker, Expectation, HasDiscoveries, Model,
    actor::{ActorModelAction, Id},
};

fn replay<M: Model>(model: &M, actions: impl IntoIterator<Item = M::Action>) -> M::State
where
    M::Action: std::fmt::Debug,
{
    let mut state = model.init_states().remove(0);
    for action in actions {
        let label = format!("{action:?}");
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
fn counterexample<M>(model: M, property: &'static str) -> String
where
    M: Model + Clone + Send + Sync + 'static,
    M::State: std::hash::Hash + Send + Sync + Clone + PartialEq + 'static,
    M::Action: Clone + PartialEq + std::fmt::Debug,
{
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
            steps.extend(action.map(|action| model.format_action(&action)));
        }
        panic!("counterexample path never violates {property:?}");
    }
    panic!("no counterexample for {property:?} within {MAX_COUNTEREXAMPLE_STEPS} steps")
}

type Action = ActorModelAction<Message, Timer, ()>;

fn deliver(src: Id, dst: Id, msg: Message) -> Action {
    ActorModelAction::Deliver { src, dst, msg }
}

fn timeout(actor: Id, timer: Timer) -> Action {
    ActorModelAction::Timeout(actor, timer)
}

fn token(epoch: u8, sequence: u8) -> Token {
    Token { epoch, sequence }
}

/// Every scenario is explored exhaustively within its bounds:
///
/// - Two orchestrators compete for x. Leases expire, renew, and are released, and
///   either orchestrator may give up before the store acknowledges its preparation.
/// - One orchestrator and the store each crash and restart once, at any point.
/// - Two orchestrators compete for x while the store restarts once, losing every
///   reservation that was not yet prepared.
///
/// Messages may be delayed, reordered, or lost throughout. Each scenario also has to
/// reach a submission, so a store that refuses everything would not pass.
#[test]
fn check_all_handshake_interleavings() {
    for scenario in [
        Scenario::Contention,
        Scenario::Recovery,
        Scenario::StoreRestart,
    ] {
        let checker = handshake::model(scenario, Fault::None)
            .checker()
            .spawn_dfs()
            .join();
        checker.assert_properties();
        println!("{scenario:?}: {} states", checker.unique_state_count());
    }
}

/// One participant at a time breaks a rule while the others follow the protocol.
/// For each, the checker must find a run violating the guarantee the rule protects:
///
/// - Expiring, releasing, or overlooking a preparation lets a second orchestrator be
///   acknowledged for the same input, as does accepting or reviving an ended lease.
/// - Acknowledging before saving, or losing saved preparations on restart, leaves an
///   acknowledgement with nothing durable behind it.
/// - Keeping a lease after answering its release lets an abandoned attempt be
///   acknowledged.
/// - Submitting without waiting for the acknowledgement submits a payload the store
///   may never have prepared.
#[test]
fn handshake_faults_have_specific_counterexamples() {
    for (scenario, fault, property) in [
        (
            Scenario::Contention,
            Fault::ExpirePrepared,
            "acknowledged attempts are exclusive",
        ),
        (
            Scenario::Contention,
            Fault::AcceptStaleReservation,
            "acknowledged attempts are exclusive",
        ),
        (
            Scenario::Contention,
            Fault::RenewExpiredLease,
            "acknowledged attempts are exclusive",
        ),
        (
            Scenario::Contention,
            Fault::ReleasePrepared,
            "acknowledged attempts are exclusive",
        ),
        (
            Scenario::Contention,
            Fault::ReserveOverPreparedInputs,
            "acknowledged attempts are exclusive",
        ),
        (
            Scenario::Contention,
            Fault::AcknowledgeBeforeCommit,
            "acknowledgements are saved",
        ),
        (
            Scenario::Recovery,
            Fault::ForgetPreparationOnRestart,
            "acknowledgements are saved",
        ),
        (
            Scenario::Contention,
            Fault::IgnoreRelease,
            "released tokens stay unacknowledged",
        ),
        (
            Scenario::Recovery,
            Fault::SubmitWithoutAcknowledgement,
            "submissions are prepared",
        ),
    ] {
        let trace = counterexample(handshake::model(scenario, fault), property);
        println!("{fault:?}: {trace}");
    }
}

/// The first orchestrator reserves x and y, proves, saves its payload, and sends a
/// preparation request. Before the request arrives, its lease ends: either the
/// lease expires, or the orchestrator gives up and releases it. The second
/// orchestrator then reserves x and z.
///
/// When the delayed request finally arrives, the store must reject it. The token's
/// lease has ended, and x now belongs to another reservation; accepting the request
/// would authorize two attempts to consume x.
#[test]
fn delayed_preparation_cannot_use_an_ended_lease() {
    let model = handshake::model(Scenario::Contention, Fault::None);
    let [first, second] = [Id::from(1), Id::from(2)];
    let first_token = token(0, 0);
    let prepare = Message::Prepare {
        token: first_token,
        outputs: Objects::P,
    };
    let reserved_and_proved = [
        deliver(
            first,
            store(),
            Message::Reserve(Objects::X.union(Objects::Y)),
        ),
        deliver(store(), first, Message::Granted(first_token)),
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
                    deliver(first, store(), prepare.clone()),
                ]),
        );
        assert!(!saved_preparations(&state).contains_key(&first_token));
        assert!(store_state(&state).leases[&token(0, 1)].active);
    }
}

/// The orchestrator's preparation is saved, but the acknowledgement never reaches
/// it. The store then crashes and restarts, followed by the orchestrator.
///
/// The restarted orchestrator does not know whether its preparation succeeded, so
/// it repeats the request from its journal. The store finds the preparation it
/// saved before the restart and acknowledges it again, and the orchestrator
/// submits. The restart lost every lease, but not the preparation.
#[test]
fn lost_acknowledgement_is_recovered_after_both_restarts() {
    let model = handshake::model(Scenario::Recovery, Fault::None);
    let orchestrator = Id::from(1);
    let prepared = token(0, 0);
    let prepare = Message::Prepare {
        token: prepared,
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
            deliver(store(), orchestrator, Message::Granted(prepared)),
            timeout(orchestrator, Timer::ProofReady),
            deliver(orchestrator, store(), prepare.clone()),
            // The preparation is saved; its acknowledgement is not delivered.
            ActorModelAction::Crash(store()),
            ActorModelAction::Recover(store()),
            ActorModelAction::Crash(orchestrator),
            ActorModelAction::Recover(orchestrator),
            deliver(orchestrator, store(), prepare),
            deliver(store(), orchestrator, Message::Prepared(prepared)),
        ],
    );
    assert_eq!(
        saved_journal(&state, orchestrator).outcome,
        Some(Outcome::Submitted)
    );
    assert!(orchestrator_state(&state, orchestrator).restarted);
    assert!(saved_preparations(&state).contains_key(&prepared));
    assert!(store_state(&state).leases.is_empty());
}

/// The first orchestrator holds a lease on x and y and has sent its preparation
/// request when the store restarts. Unprepared reservations are not saved, so the
/// lease is gone, and the second orchestrator reserves x and z under a token from
/// the new epoch.
///
/// The first orchestrator's request then arrives with its old token and is
/// rejected. It reserves again, but x is taken, so it gives up. The second
/// orchestrator prepares and submits. Forgetting the reservation cost the first
/// orchestrator its attempt, but never let both attempts hold x.
#[test]
fn store_restart_ends_unprepared_reservations_without_sharing_inputs() {
    let model = handshake::model(Scenario::StoreRestart, Fault::None);
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
            deliver(store(), first, Message::Granted(first_token)),
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
                Message::Prepare {
                    token: first_token,
                    outputs: Objects::P,
                },
            ),
            deliver(store(), first, Message::PrepareRejected(first_token)),
            deliver(
                first,
                store(),
                Message::Reserve(Objects::X.union(Objects::Y)),
            ),
            deliver(store(), first, Message::Refused),
            deliver(store(), second, Message::Granted(second_token)),
            timeout(second, Timer::ProofReady),
            deliver(
                second,
                store(),
                Message::Prepare {
                    token: second_token,
                    outputs: Objects::Q,
                },
            ),
            deliver(store(), second, Message::Prepared(second_token)),
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
        saved_preparations(&state).keys().collect::<Vec<_>>(),
        [&second_token]
    );
}
