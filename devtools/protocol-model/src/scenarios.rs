use stateright::{Expectation, actor::Id};

use crate::{
    handshake::{
        self, HandshakeModel, HandshakeState, Orchestrator, OrchestratorRules, Outcome,
        Participant, ProtocolOrchestrator, ProtocolStore, Store, StoreRules, Synchronizer,
        orchestrator_state, orchestrators_of, saved_journal, store_state,
    },
    objects::Objects,
};

/// The store restarts explored; each restart begins a new reservation epoch.
const MAX_EPOCH: u8 = 1;
/// The reservations the store may grant within one epoch.
const MAX_GRANTS_PER_EPOCH: usize = 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scenario {
    /// Two orchestrators compete for x, renewing their leases and possibly giving up;
    /// nobody crashes.
    Contention,
    /// One orchestrator; the store and the orchestrator each crash and restart at
    /// most once.
    Recovery,
    /// Two orchestrators compete for x while the store may restart once.
    StoreRestart,
}

pub type ScenarioModel = HandshakeModel<Scenario>;

pub fn store() -> Id {
    Id::from(0)
}

fn orchestrator_count(scenario: Scenario) -> usize {
    if scenario == Scenario::Recovery { 1 } else { 2 }
}

/// The orchestrators follow the store. The first reserves x and y to create p; when
/// there is a second, it reserves x and z to create q, so the two compete for x.
pub fn orchestrators(scenario: Scenario) -> impl Iterator<Item = Id> {
    (1..=orchestrator_count(scenario)).map(Id::from)
}

/// The synchronizer follows the orchestrators.
pub fn synchronizer(scenario: Scenario) -> Id {
    Id::from(orchestrator_count(scenario) + 1)
}

fn orchestrator(id: Id, scenario: Scenario, rules: &'static dyn OrchestratorRules) -> Orchestrator {
    let first = orchestrators(scenario).next() == Some(id);
    Orchestrator {
        store: store(),
        inputs: Objects::X.union(if first { Objects::Y } else { Objects::Z }),
        outputs: if first { Objects::P } else { Objects::Q },
        cancellable: scenario == Scenario::Contention,
        rules,
    }
}

/// The store restarts at most once. An orchestrator crashes only in the recovery
/// scenario, and at most once. The synchronizer never crashes.
fn within_bounds(scenario: &Scenario, state: &HandshakeState) -> bool {
    let scenario = *scenario;
    let current = store_state(state, store());
    let store_crashed_again =
        state.crashed[usize::from(store())] && current.saved.epoch >= MAX_EPOCH;
    let orchestrator_crash_excluded = orchestrators(scenario).any(|orchestrator| {
        state.crashed[usize::from(orchestrator)]
            && (scenario != Scenario::Recovery || orchestrator_state(state, orchestrator).restarted)
    });
    !store_crashed_again
        && !orchestrator_crash_excluded
        && !state.crashed[usize::from(synchronizer(scenario))]
        && current.leases.len() <= MAX_GRANTS_PER_EPOCH
}

fn submitted(state: &HandshakeState, orchestrator: Id) -> bool {
    saved_journal(state, orchestrator).outcome == Some(Outcome::Submitted)
}

/// A settlement freed an input that another transaction then took.
fn acknowledgement_after_a_settlement(_: &ScenarioModel, state: &HandshakeState) -> bool {
    let history = &state.history;
    history.acknowledged.values().any(|&acknowledged_at| {
        history
            .settled
            .values()
            .any(|(settled_at, _)| *settled_at < acknowledged_at)
    })
}

fn submission_under_a_reservation_from_after_a_store_restart(
    model: &ScenarioModel,
    state: &HandshakeState,
) -> bool {
    orchestrators_of(model).any(|(orchestrator, _)| {
        submitted(state, orchestrator)
            && saved_journal(state, orchestrator)
                .reservation
                .is_some_and(|reservation| reservation.epoch > 0)
    })
}

fn submission_by_a_restarted_orchestrator(model: &ScenarioModel, state: &HandshakeState) -> bool {
    orchestrators_of(model).any(|(orchestrator, _)| {
        submitted(state, orchestrator) && orchestrator_state(state, orchestrator).restarted
    })
}

/// The store and the scenario's orchestrators, all following the protocol.
pub fn model(scenario: Scenario) -> ScenarioModel {
    with_rules(scenario, &ProtocolStore, &ProtocolOrchestrator)
}

/// The scenario with the store and every orchestrator following the given rules.
pub fn with_rules(
    scenario: Scenario,
    store_rules: &'static dyn StoreRules,
    orchestrator_rules: &'static dyn OrchestratorRules,
) -> ScenarioModel {
    let participants = std::iter::once(Participant::Store(Store {
        rules: store_rules,
        synchronizer: synchronizer(scenario),
    }))
    .chain(
        orchestrators(scenario)
            .map(|id| Participant::Orchestrator(orchestrator(id, scenario, orchestrator_rules))),
    )
    .chain(std::iter::once(Participant::Synchronizer(Synchronizer)));
    let mut model = handshake::system(scenario, participants)
        .max_crashes(if scenario == Scenario::Contention {
            0
        } else {
            1
        })
        .within_boundary(within_bounds);
    if scenario == Scenario::Recovery {
        model = model.property(
            Expectation::Sometimes,
            "submission by a restarted orchestrator",
            submission_by_a_restarted_orchestrator,
        );
    } else {
        model = model.property(
            Expectation::Sometimes,
            "acknowledgement after a settlement",
            acknowledgement_after_a_settlement,
        );
    }
    if scenario != Scenario::Contention {
        model = model.property(
            Expectation::Sometimes,
            "submission under a reservation from after a store restart",
            submission_under_a_reservation_from_after_a_store_restart,
        );
    }
    model
}
