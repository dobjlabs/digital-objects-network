use protocol_model::{
    effects::{Coverage, Objects},
    lifecycle::{
        Attempt, Fault as LifecycleFault, GENESIS, Inclusion, LifecycleModel, LifecycleStep,
        Scenario, Transaction,
    },
    reservations::{Fault as ReservationFault, ReservationModel, ReservationStep},
};
use stateright::{Checker, Expectation, HasDiscoveries, Model};

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

/// Searches breadth-first for a violation of the named safety property and returns
/// the steps leading to the first violating state on the path found.
fn counterexample<M>(model: M, property: &'static str) -> String
where
    M: Model + Send + Sync + 'static,
    M::State: std::hash::Hash + Send + Sync + Clone + PartialEq + 'static,
    M::Action: Clone + PartialEq + std::fmt::Debug,
{
    let checker = model
        .checker()
        .finish_when(HasDiscoveries::AnyOf([property].into()))
        .spawn_bfs()
        .join();
    let path = checker
        .discoveries()
        .remove(property)
        .unwrap_or_else(|| panic!("no counterexample for {property:?}"));
    let model = checker.model();
    let safety = model
        .properties()
        .into_iter()
        .find(|candidate| {
            candidate.name == property && candidate.expectation == Expectation::Always
        })
        .unwrap_or_else(|| panic!("no safety property named {property:?}"));
    let mut steps = Vec::new();
    for (state, action) in path.into_vec() {
        if !(safety.condition)(model, &state) {
            return steps.join(", ");
        }
        steps.extend(action.map(|action| model.format_action(&action)));
    }
    panic!("counterexample path never violates {property:?}")
}

/// Two orchestrators request overlapping input sets from the same store. Lease
/// renewal, expiry, release, and preparation can occur in different orders, but
/// neither orchestrator may obtain authority over inputs still held by the other.
/// Once preparation commits, the original lease deadline no longer ends protection.
///
/// A separate recovery scenario allows the store and one orchestrator to crash and
/// restart. Replies may arrive after recovery, but any acknowledgement must still
/// refer to retained preparation data, and submission must have a saved payload.
///
/// The checker explores all reachable orderings within these bounded scenarios.
/// It also requires examples of successful submission and recovery, so simply
/// preventing every submission would not satisfy the checks.
#[test]
fn check_all_reservation_interleavings() {
    for (name, model) in [
        ("leases", ReservationModel::leases()),
        ("recovery", ReservationModel::recovery()),
    ] {
        let checker = model.checker().spawn_dfs().join();
        checker.assert_properties();
        println!("{name}: {} states", checker.unique_state_count());
    }
}

/// A prepared local attempt consumes x and y to create p. Its subsequent history
/// is explored in three bounded settings:
///
/// - Other transactions produce some, all, or additional effects relative to the intent.
/// - A dependent attempt consumes p before finality, and a reorg can remove its grounding.
/// - Chain progress makes the original grounding too old without producing its effects.
///
/// In each setting, the chain, synchronizer, and store may progress at different
/// times. The store must use the view it has received while retaining the records
/// and protection needed for unresolved attempts. The checker explores every
/// reachable ordering and requires witnesses for the intended useful outcomes;
/// those witnesses demonstrate possibility, not guaranteed eventual progress.
#[test]
fn check_all_lifecycle_interleavings() {
    for scenario in [Scenario::Effects, Scenario::Reorg, Scenario::Expiry] {
        let checker = LifecycleModel::new(scenario).checker().spawn_dfs().join();
        checker.assert_properties();
        println!("{scenario:?}: {} states", checker.unique_state_count());
    }
}

/// The store is allowed one incorrect behavior at a time while orchestrators
/// continue following the protocol. Each error breaks a different part of the
/// reservation and preparation handshake:
///
/// - Expiring a prepared lease removes protection after submission was authorized.
/// - An early acknowledgement promises recovery data that has not yet been committed.
/// - Accepting a stale token mistakes another reservation's authority for its own.
/// - Renewing an expired lease revives authority that a pending request can then use.
/// - Releasing a prepared reservation removes protection an acknowledgement relies on.
/// - Ignoring a prepared attempt once its lease expires gives its inputs to another reservation.
/// - Losing a committed preparation on crash invalidates an acknowledgement already sent.
///
/// The checker must find an execution violating the guarantee paired with that
/// error. An unrelated failure does not count. These counterexamples demonstrate
/// that the properties distinguish the correct handshake from these broken ones.
#[test]
fn reservation_mutations_have_specific_counterexamples() {
    for (fault, property) in [
        (
            ReservationFault::ExpirePrepared,
            "prepared inputs remain protected",
        ),
        (
            ReservationFault::AcknowledgeBeforeCommit,
            "acknowledgements have durable preparations",
        ),
        (
            ReservationFault::AcceptStaleReservation,
            "stale reservations cannot prepare",
        ),
        (
            ReservationFault::RenewExpiredLease,
            "stale reservations cannot prepare",
        ),
        (
            ReservationFault::ReleasePrepared,
            "prepared inputs remain protected",
        ),
        (
            ReservationFault::ReserveOverPreparedInputs,
            "reservations are exclusive",
        ),
        (
            ReservationFault::ForgetPreparationOnRestart,
            "acknowledgements have durable preparations",
        ),
    ] {
        let mut model = if fault == ReservationFault::ForgetPreparationOnRestart {
            ReservationModel::recovery()
        } else {
            ReservationModel::leases()
        };
        model.fault = fault;
        println!("{fault:?}: {}", counterexample(model, property));
    }
}

/// The store follows a chain where a parent transaction and its dependent can be
/// displaced by a competing branch. Observations can arrive after the chain has
/// changed again. Five separate errors are introduced into the store's handling:
///
/// - Releasing inputs on observation treats a reversible conflict as permanent.
/// - Measuring grounding expiry from the observed head releases inputs that the
///   original payload can still consume after a rollback.
/// - Accepting an older snapshot lets a late response undo a newer reconciliation.
/// - Retaining orphan availability makes an object spendable after its creation disappears.
/// - Permitting cleanup on observation allows data loss before consumption is final.
///
/// For each error, the checker must find an execution violating the corresponding
/// guarantee. This establishes that the checks can detect these mistakes even when
/// the triggering chain event and its delivery to the store happen at different times.
#[test]
fn lifecycle_mutations_have_specific_counterexamples() {
    for (fault, property) in [
        (
            LifecycleFault::ReleaseOnObservation,
            "released attempts cannot apply",
        ),
        (
            LifecycleFault::ExpireOnObservedHead,
            "released attempts cannot apply",
        ),
        (
            LifecycleFault::AcceptOldObservations,
            "old observations cannot replace newer ones",
        ),
        (
            LifecycleFault::KeepOrphanOutputsAvailable,
            "availability follows the applied observation",
        ),
        (
            LifecycleFault::CleanupOnObservation,
            "cleanup requires finalized consumption",
        ),
    ] {
        let mut model = LifecycleModel::new(Scenario::Reorg);
        model.fault = fault;
        println!("{fault:?}: {}", counterexample(model, property));
    }
}

/// The first orchestrator reserves x and y, saves its payload, and requests
/// preparation. The store has received the request but has not committed it yet.
/// While it is pending, either the orchestrator releases its lease or time passes
/// until the lease expires. The test follows both possibilities.
///
/// A second orchestrator then reserves x and z. When the store finally processes
/// the first preparation request, it must check that request's token again. The
/// fact that x is currently reserved does not help: it now belongs to a different
/// reservation. Accepting the old request would authorize two attempts to consume x.
///
/// The old attempt remains unprepared. It also cannot renew the ended lease or
/// acquire another lease under its old identity; neither operation may revive the
/// authority carried by a delayed request.
#[test]
fn pending_preparation_cannot_commit_after_release_or_expiry() {
    use ReservationStep::*;
    let model = ReservationModel::leases();
    let prefix = [
        Reserve(0),
        ReceiveGrant(0),
        JournalProof(0),
        RequestPrepare(0),
    ];
    for invalidation in [vec![Release(0)], vec![Tick, Tick]] {
        let state = replay(
            &model,
            prefix
                .clone()
                .into_iter()
                .chain(invalidation)
                .chain([Reserve(1), CommitPrepare(0)]),
        );
        assert!(!state.attempts[0].reservation.prepared);
        assert!(model.next_state(&state, Renew(0)).is_none());
        assert!(model.next_state(&state, Reserve(0)).is_none());
    }
}

/// The orchestrator reserves inputs, saves its payload, and asks the store to
/// prepare. The store commits the preparation and output contents, but both
/// processes crash before an acknowledgement reaches the orchestrator. In this
/// trace the reply was not sent before the crash; recovery must still obtain it.
///
/// The lease deadline passes while the processes are down. Because preparation
/// already committed, this deadline cannot release the inputs. After restarting,
/// the orchestrator cannot tell whether its request committed, so it resends it
/// from its journal. The store keeps the preparation it recorded, confirms it, and
/// the orchestrator submits the payload recovered from its journal.
///
/// The final state must still contain the journal and input protection. Once a
/// delayed copy of the grant gives the orchestrator its token again, an explicit
/// release reaches the store, which must reject it for a prepared reservation.
#[test]
fn lost_acknowledgement_is_recovered_after_both_restarts() {
    use ReservationStep::*;
    let model = ReservationModel::recovery();
    let state = replay(
        &model,
        [
            Reserve(0),
            ReceiveGrant(0),
            JournalProof(0),
            RequestPrepare(0),
            CommitPrepare(0),
            // The preparation is durable, but the orchestrator has no acknowledgement.
            CrashStore,
            CrashActor(0),
            Tick,
            Tick,
            RestartStore,
            RestartActor(0),
            // The retried request finds the preparation already recorded, and the
            // store confirms it after the lease deadline.
            RequestPrepare(0),
            CommitPrepare(0),
            SendAck(0),
            ReceiveAck(0),
            Submit(0),
            ReceiveGrant(0),
        ],
    );
    assert!(state.attempts[0].orchestrator.submitted);
    assert!(state.attempts[0].reservation.protected);
    assert!(state.attempts[0].orchestrator.journal);
    assert!(state.attempts[0].orchestrator.grant_received);
    assert!(model.next_state(&state, Release(0)).is_none());
}

/// The orchestrator has saved its payload and sent a preparation request, but the
/// store crashes before committing it. The pending request was volatile, so after
/// restart there is neither a preparation to acknowledge nor a pending request
/// that the store can finish processing.
///
/// The lease remains valid in this trace. The orchestrator can therefore resend
/// the request using its existing token and payload. Only after that request is
/// committed and acknowledged can submission occur. This distinguishes recovery
/// before the durable boundary from recovery of an already committed preparation.
#[test]
fn crash_before_commit_requires_retry_and_cannot_acknowledge() {
    use ReservationStep::*;
    let model = ReservationModel::recovery();
    let mut state = replay(
        &model,
        [
            Reserve(0),
            ReceiveGrant(0),
            JournalProof(0),
            RequestPrepare(0),
            CrashStore,
            RestartStore,
        ],
    );
    assert!(model.next_state(&state, SendAck(0)).is_none());
    assert!(model.next_state(&state, CommitPrepare(0)).is_none());
    // The lease is still valid when the orchestrator resends its preparation request.
    for action in [
        RequestPrepare(0),
        CommitPrepare(0),
        SendAck(0),
        ReceiveAck(0),
        Submit(0),
    ] {
        state = model.next_state(&state, action).unwrap();
    }
    assert!(state.attempts[0].orchestrator.submitted);
}

/// The parent transaction consumes x and y to create p, and the store observes it
/// before finality. The orchestrator then prepares and submits a dependent attempt
/// consuming p to create q. Its saved proof is grounded in the block that created p.
///
/// Before the dependent payload is included, a reorg removes the parent transaction.
/// The replacement branch consumes x to create r instead. The dependent payload
/// still reaches a later block, but cannot apply there: its original grounding
/// belongs to the discarded branch, and p has not been created on the replacement.
///
/// The synchronizer rolls back and processes the replacement blocks before the
/// store receives its latest snapshot. The resulting local state distinguishes:
///
/// - Partial coverage of the parent intent, because only its consumption of x occurred.
/// - No coverage of the dependent intent, whose payload was rejected entirely.
/// - Retained preparations, proof grounding, and output contents for recovery.
/// - Unavailable p and q, since neither creation exists in the store's new view.
///
/// The replacement branch is still unfinalized. At the protocol level a later
/// reorg could restore the original history, so neither attempt's input protection
/// can end on this evidence alone.
#[test]
fn dependent_proof_survives_locally_but_fails_on_replacement_branch() {
    use Attempt::{Dependent, Parent};
    use LifecycleStep::*;
    let model = LifecycleModel::new(Scenario::Reorg);
    let parent_block = model.block("parent");
    let competitor = model.block("competitor");
    let dependent_on_competitor = model.block("dependent-on-replacement-branch");
    let state = replay(
        &model,
        [
            // The store observes the parent transaction's creation of p.
            ReceivePreparation(Parent),
            Submit(Parent),
            Extend(parent_block),
            ProcessSynchronizer,
            DeliverObservation(0),
            // The dependent attempt consumes p and saves a proof grounded in the parent's block.
            JournalProof(Dependent),
            Prepare(Dependent),
            ReceivePreparation(Dependent),
            Submit(Dependent),
            // A competing spend of x replaces the parent; the saved dependent payload
            // appears on that branch but cannot apply without its original grounding.
            ReorgTo(GENESIS),
            Extend(competitor),
            Extend(dependent_on_competitor),
            // The synchronizer returns to genesis, then processes both replacement blocks.
            ProcessSynchronizer,
            ProcessSynchronizer,
            ProcessSynchronizer,
            // The store receives the latest view without receiving the intermediate ones.
            DeliverObservation(3),
        ],
    );
    assert_eq!(state.attempts[Parent].coverage, Coverage::Partial);
    assert_eq!(state.attempts[Dependent].coverage, Coverage::None);
    // The unfinalized replacement branch does not yet justify releasing either
    // attempt's inputs. The store also retains the prepared output contents,
    // although p and q are absent from its current chain view and unavailable.
    assert!(state.attempts[Parent].protected() && state.attempts[Dependent].protected());
    assert_eq!(state.attempts[Dependent].journal, Some(parent_block));
    assert!(
        state
            .ledger(dependent_on_competitor)
            .rejected_local
            .contains(Dependent)
    );
    assert!(!state.available.overlaps(Objects::P.union(Objects::Q)));
    assert!(state.contents.contains(Objects::P.union(Objects::Q)));
}

/// The orchestrator has prepared and submitted a transaction that consumes x and y
/// and creates p. The store observes it on the chain, but its input protection
/// remains in place because that observation is not yet final.
///
/// A reorg replaces this transaction with a competing transaction that consumes
/// only x and creates r. The replacement branch then becomes finalized. The
/// original payload can no longer apply: it would have to consume x a second time,
/// and the finalized consumption of x cannot be removed by another reorg.
///
/// When the store receives this finalized view, it can stop protecting the original
/// attempt's inputs. This resolves the attempt without achieving its intended effects:
///
/// - y becomes available again because the replacement transaction never consumed it.
/// - x remains unavailable because its consumption is now finalized.
/// - p remains unavailable because its creation disappeared with the original branch.
///
/// Finally, the store receives a delayed copy of the earlier observation, in which
/// the original transaction had succeeded. It must ignore that older view: accepting
/// it would incorrectly make p available and mark y as consumed again.
///
/// Finality is also what keeps the original branch from returning. A separate trace
/// that has not spent its one rewind can roll the competing block back to genesis
/// while it is unfinalized, but not once it is finalized.
#[test]
fn finalized_conflict_releases_remaining_inputs_and_old_observations_cannot_undo_it() {
    use Attempt::Parent;
    use LifecycleStep::*;
    let model = LifecycleModel::new(Scenario::Reorg);
    let parent_block = model.block("parent");
    let competitor = model.block("competitor");
    let state = replay(
        &model,
        [
            ReceivePreparation(Parent),
            Submit(Parent),
            Extend(parent_block),
            ProcessSynchronizer,
            DeliverObservation(0),
            // The chain switches branches and finalizes the competing spend before
            // either the synchronizer or the store learns about the replacement.
            ReorgTo(GENESIS),
            Extend(competitor),
            Finalize(competitor),
            // The synchronizer first returns to genesis, then processes the finalized
            // replacement. The store receives that view as observation 2.
            ProcessSynchronizer,
            ProcessSynchronizer,
            DeliverObservation(2),
        ],
    );
    assert_eq!(state.attempts[Parent].coverage, Coverage::Partial);
    assert!(!state.attempts[Parent].protected());
    assert!(state.available.contains(Objects::Y));
    assert!(!state.available.overlaps(Objects::X.union(Objects::P)));
    // Observation 0 still describes the original transaction's apparent success.
    // Its late delivery must leave the reconciled state unchanged.
    assert!(model.next_state(&state, DeliverObservation(0)).is_none());

    let unfinalized_competitor = replay(&model, [Extend(competitor)]);
    assert!(
        model
            .next_state(&unfinalized_competitor, ReorgTo(GENESIS))
            .is_some()
    );
    let finalized_competitor = replay(&model, [Extend(competitor), Finalize(competitor)]);
    assert!(
        model
            .next_state(&finalized_competitor, ReorgTo(GENESIS))
            .is_none()
    );
}

/// The local orchestrator prepares a transaction consuming x and y to create p.
/// Another holder has a copy of x and submits a transaction consuming it to create
/// r. Our store's reservation cannot prevent that independent submission. The
/// other holder is represented by an external payload, not a second store process.
///
/// Both payloads reach the same block. In either ordering, only the first applies:
/// the second conflicts with the nullifier of x and contributes no effects. The
/// local store then learns which transaction won, but retains input protection
/// until it receives finalized evidence of that outcome.
///
/// - If the local transaction wins, p is available and both x and y are consumed.
/// - If the competitor wins, p was never created and y remains unspent. The store
///   releases y only after learning that the competing consumption of x is final.
///
/// In both cases the preparation and saved payload remain, along with p's local
/// contents. A losing attempt's retained output data does not make its output live.
#[test]
fn prepared_attempt_races_an_independent_spend_and_reconciles_either_winner() {
    use Attempt::Parent;
    use Inclusion::{External, Local};
    use LifecycleStep::*;
    use protocol_model::effects::Effects;

    let competitor = Transaction::consuming(Objects::X, Objects::R);
    for local_wins in [true, false] {
        let mut model = LifecycleModel::new(Scenario::Effects);
        let block = model.add_block(
            "competing-submissions",
            GENESIS,
            if local_wins {
                vec![Local(Parent), External(competitor)]
            } else {
                vec![External(competitor), Local(Parent)]
            },
        );
        let observed_trace = [
            ReceivePreparation(Parent),
            Submit(Parent),
            Extend(block),
            ProcessSynchronizer,
            DeliverObservation(0),
        ];
        let observed = replay(&model, observed_trace.clone());
        let winner = if local_wins {
            Transaction::parent()
        } else {
            competitor
        };
        let coverage = if local_wins {
            Coverage::Full
        } else {
            Coverage::Partial
        };
        assert_eq!(
            observed.ledger(block).effects,
            Effects {
                created: Objects::INITIAL.union(winner.effects.created),
                nullifiers: winner.effects.nullifiers,
            }
        );
        assert_eq!(
            observed.ledger(block).accepted_local.contains(Parent),
            local_wins
        );
        assert_eq!(
            observed.ledger(block).rejected_local.contains(Parent),
            !local_wins
        );
        assert_eq!(observed.attempts[Parent].coverage, coverage);
        assert!(observed.attempts[Parent].protected());
        assert!(!observed.available.overlaps(Objects::X.union(Objects::Y)));
        assert_eq!(observed.available.contains(Objects::P), local_wins);

        // Finality on the chain does not release inputs until the store learns of it.
        let finalized_trace = observed_trace.into_iter().chain([Finalize(block)]);
        let awaiting_observation = replay(&model, finalized_trace.clone());
        assert!(awaiting_observation.attempts[Parent].protected());
        let finalized = replay(
            &model,
            finalized_trace.chain([ProcessSynchronizer, DeliverObservation(1)]),
        );
        assert_eq!(finalized.attempts[Parent].coverage, coverage);
        assert!(!finalized.attempts[Parent].protected());
        assert_eq!(
            finalized.available,
            Objects::Z.union(if local_wins { Objects::P } else { Objects::Y })
        );
        assert!(finalized.attempts[Parent].prepared());
        assert_eq!(finalized.attempts[Parent].journal, Some(GENESIS));
        assert!(
            finalized
                .contents
                .contains(Objects::INITIAL.union(Objects::P))
        );
    }
}

/// The orchestrator submits a prepared attempt consuming x and y to create p,
/// grounded at genesis. An empty block passes before a second block carries the
/// payload. With a grounding age limit of one block, the payload is now too old
/// and is rejected without producing any effects.
///
/// Once that progress is finalized and observed by the store, the exact saved
/// attempt can never apply. Its input protection ends, x and y become available,
/// and p remains unavailable because it was never created. This is resolution by
/// expiry rather than by a conflicting spend or successful execution. Protection
/// can end one block earlier, before the payload is even rejected: once the empty
/// block is finalized, its grounding is too old for every later block. Before that
/// finality it must remain.
///
/// The orchestrator then saves a replacement proof grounded in the newer block.
/// Although it has the same intended effects, it is a different submission attempt.
/// The old acknowledgement still binds the genesis-grounded payload and must not
/// authorize the replacement. Only after its own preparation and acknowledgement
/// is the replacement submitted, and the next block applies it.
///
/// A faulty variant allows that reuse. The checker must find a submission that
/// violates grounding-bound authorization, demonstrating why matching effects
/// alone are insufficient.
#[test]
fn expired_preparation_releases_inputs_but_cannot_authorize_a_new_grounding() {
    use Attempt::{Parent, Replacement};
    use LifecycleStep::*;
    let model = LifecycleModel::new(Scenario::Expiry);
    let empty = model.block("empty");
    let expired_parent = model.block("expired-parent");
    let replacement_block = model.block("replacement");
    let unfinalized = replay(
        &model,
        [Extend(empty), ProcessSynchronizer, DeliverObservation(0)],
    );
    assert!(unfinalized.attempts[Parent].protected());
    let window_closed = replay(
        &model,
        [
            Extend(empty),
            Finalize(empty),
            ProcessSynchronizer,
            DeliverObservation(0),
        ],
    );
    assert!(!window_closed.attempts[Parent].protected());

    let resolved = [
        ReceivePreparation(Parent),
        Submit(Parent),
        // The payload arrives after its grounding window, so its block rejects it.
        Extend(empty),
        Extend(expired_parent),
        Finalize(expired_parent),
        ProcessSynchronizer,
        ProcessSynchronizer,
        DeliverObservation(1),
    ];
    let state = replay(&model, resolved.clone());
    assert_eq!(state.attempts[Parent].coverage, Coverage::None);
    assert!(!state.attempts[Parent].protected());
    assert!(state.available.contains(Objects::INITIAL));
    assert!(!state.available.overlaps(Objects::P));
    // The replacement is grounded in the newer block; the retained acknowledgement binds genesis.
    let journaled = replay(
        &model,
        resolved
            .clone()
            .into_iter()
            .chain([JournalProof(Replacement)]),
    );
    assert_eq!(
        journaled.attempts[Replacement].journal,
        Some(expired_parent)
    );
    assert!(model.next_state(&journaled, Submit(Replacement)).is_none());
    let applied = replay(
        &model,
        resolved.into_iter().chain([
            JournalProof(Replacement),
            Prepare(Replacement),
            ReceivePreparation(Replacement),
            Submit(Replacement),
            Extend(replacement_block),
        ]),
    );
    assert!(
        applied
            .ledger(replacement_block)
            .accepted_local
            .contains(Replacement)
    );
    assert_eq!(applied.attempts[Parent].journal, Some(GENESIS));

    let mut broken = model;
    broken.fault = LifecycleFault::AuthorizeReplacementWithOldAck;
    let trace = counterexample(broken, "submitted payloads have matching preparations");
    println!("AuthorizeReplacementWithOldAck: {trace}");
}

/// Two independent payloads arrive in the same block. One consumes x to create p;
/// the other consumes x and y to create q and r. Both can be grounded in the
/// original state, but they cannot both apply because each consumes x.
///
/// The block is evaluated in both possible orders:
///
/// - If the first payload wins, only p is created and only x is consumed. The losing
///   payload must not consume y or create either q or r, despite those effects not
///   themselves conflicting with the winner.
/// - If the second wins, q and r are created and x and y are consumed. The losing
///   payload must not create p.
///
/// Comparing the entire resulting effect set with the winner's effects checks
/// atomic rejection of the loser. This isolates block application; neither payload
/// has a local preparation whose subsequent reconciliation is involved.
#[test]
fn synchronizer_applies_payloads_in_order_without_partial_effects() {
    use protocol_model::effects::Effects;
    let first = Transaction::consuming(Objects::X, Objects::P);
    let second = Transaction::consuming(Objects::X.union(Objects::Y), Objects::Q.union(Objects::R));
    for transactions in [[first, second], [second, first]] {
        let mut model = LifecycleModel::new(Scenario::Effects);
        let block = model.add_block(
            "ordered-conflicts",
            GENESIS,
            transactions.into_iter().map(Inclusion::External).collect(),
        );
        let state = replay(&model, [LifecycleStep::Extend(block)]);
        let expected = Effects {
            created: Objects::INITIAL.union(transactions[0].effects.created),
            nullifiers: transactions[0].effects.nullifiers,
        };
        assert_eq!(state.ledger(block).effects, expected);
    }
}
