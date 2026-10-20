use stateright::{Expectation, actor::Id};

use crate::{
    handshake::{
        self, HandshakeModel, HandshakeState, Orchestrator, OrchestratorRules, Outcome,
        Participant, ProtocolOrchestrator, ProtocolStore, Store, StoreRules, orchestrator_state,
        orchestrators_of, saved_journal, store_state,
    },
    objects::Objects,
};

/// The store restarts explored; each restart begins a new token epoch.
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

/// The first orchestrator reserves x and y to create p. When there is a second, it
/// reserves x and z to create q, so the two compete for x.
pub fn orchestrators(scenario: Scenario) -> Vec<Id> {
    let count = if scenario == Scenario::Recovery { 1 } else { 2 };
    (1..=count).map(Id::from).collect()
}

fn orchestrator(id: Id, scenario: Scenario, rules: &'static dyn OrchestratorRules) -> Orchestrator {
    let first = id == orchestrators(scenario)[0];
    Orchestrator {
        store: store(),
        inputs: Objects::X.union(if first { Objects::Y } else { Objects::Z }),
        outputs: if first { Objects::P } else { Objects::Q },
        cancellable: scenario == Scenario::Contention,
        rules,
    }
}

/// The store restarts at most once. An orchestrator crashes only in the recovery
/// scenario, and at most once.
fn within_bounds(scenario: &Scenario, state: &HandshakeState) -> bool {
    let current = store_state(state, store());
    let store_crashed_again =
        state.crashed[usize::from(store())] && current.saved.epoch >= MAX_EPOCH;
    let orchestrator_crash_excluded = orchestrators(*scenario).into_iter().any(|orchestrator| {
        state.crashed[usize::from(orchestrator)]
            && (*scenario != Scenario::Recovery
                || orchestrator_state(state, orchestrator).restarted)
    });
    !store_crashed_again
        && !orchestrator_crash_excluded
        && current.leases.len() <= MAX_GRANTS_PER_EPOCH
}

fn submitted(state: &HandshakeState, orchestrator: Id) -> bool {
    saved_journal(state, orchestrator).outcome == Some(Outcome::Submitted)
}

fn an_orchestrator_submits(model: &ScenarioModel, state: &HandshakeState) -> bool {
    orchestrators_of(model).any(|(orchestrator, _)| submitted(state, orchestrator))
}

fn submission_under_a_token_from_after_a_store_restart(
    model: &ScenarioModel,
    state: &HandshakeState,
) -> bool {
    orchestrators_of(model).any(|(orchestrator, _)| {
        submitted(state, orchestrator)
            && saved_journal(state, orchestrator)
                .reservation
                .is_some_and(|token| token.epoch > 0)
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
    let participants = std::iter::once(Participant::Store(Store { rules: store_rules })).chain(
        orchestrators(scenario)
            .into_iter()
            .map(|id| Participant::Orchestrator(orchestrator(id, scenario, orchestrator_rules))),
    );
    let mut model = handshake::system(scenario, participants)
        .max_crashes(if scenario == Scenario::Contention {
            0
        } else {
            1
        })
        .within_boundary(within_bounds)
        .property(
            Expectation::Sometimes,
            "an orchestrator submits",
            an_orchestrator_submits,
        );
    if scenario == Scenario::Recovery {
        model = model.property(
            Expectation::Sometimes,
            "submission by a restarted orchestrator",
            submission_by_a_restarted_orchestrator,
        );
    }
    if scenario != Scenario::Contention {
        model = model.property(
            Expectation::Sometimes,
            "submission under a token from after a store restart",
            submission_under_a_token_from_after_a_store_restart,
        );
    }
    model
}
