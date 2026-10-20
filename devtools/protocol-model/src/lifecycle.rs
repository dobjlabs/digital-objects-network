use std::{
    fmt,
    ops::{Index, IndexMut},
};

use stateright::{Model, Property};

use crate::effects::{Coverage, Effects, Objects};

pub const GENESIS: usize = 0;

/// Failure modes in reconciliation and submission authorization.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Fault {
    /// The participants follow the protocol without an injected failure.
    #[default]
    None,
    /// The store releases prepared inputs on observed effects without waiting for finality.
    ReleaseOnObservation,
    /// The store measures grounding expiry from its observed head instead of finalized progress.
    ExpireOnObservedHead,
    /// The store lets a delayed older observation overwrite its newer chain view.
    AcceptOldObservations,
    /// The store leaves an output available after learning that its creation was reorged away.
    KeepOrphanOutputsAvailable,
    /// The store permits content cleanup before the object's consumption is finalized.
    CleanupOnObservation,
    /// The orchestrator submits a newly grounded proof using the old attempt's authorization.
    AuthorizeReplacementWithOldAck,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Transaction {
    pub inputs: Objects,
    pub effects: Effects,
}

impl Transaction {
    pub fn consuming(inputs: Objects, created: Objects) -> Self {
        Self {
            inputs,
            effects: Effects {
                created,
                nullifiers: inputs,
            },
        }
    }

    pub fn parent() -> Self {
        Self::consuming(Objects::X.union(Objects::Y), Objects::P)
    }

    pub fn dependent() -> Self {
        Self::consuming(Objects::P, Objects::Q)
    }
}

/// A local submission attempt.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Attempt {
    /// Consumes x and y to create p; prepared before the model starts.
    Parent,
    /// Consumes the parent's output p to create q.
    Dependent,
    /// Has the parent's effects under a newer grounding.
    Replacement,
}

impl Attempt {
    pub const ALL: [Self; 3] = [Self::Parent, Self::Dependent, Self::Replacement];

    pub fn transaction(self) -> Transaction {
        match self {
            Self::Parent | Self::Replacement => Transaction::parent(),
            Self::Dependent => Transaction::dependent(),
        }
    }
}

#[derive(Clone, Copy, Default, Eq, Hash, PartialEq)]
pub struct AttemptSet(u8);

impl AttemptSet {
    pub fn contains(self, attempt: Attempt) -> bool {
        self.0 & (1 << attempt as u8) != 0
    }

    fn insert(&mut self, attempt: Attempt) {
        self.0 |= 1 << attempt as u8;
    }
}

impl fmt::Debug for AttemptSet {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter
            .debug_set()
            .entries(
                Attempt::ALL
                    .into_iter()
                    .filter(|attempt| self.contains(*attempt)),
            )
            .finish()
    }
}

#[derive(Clone, Debug)]
pub enum Inclusion {
    /// A block carries the saved payload of a local attempt.
    Local(Attempt),
    /// A block carries an independent holder's transaction, outside local reservations,
    /// grounded at the block's parent.
    External(Transaction),
}

#[derive(Clone, Debug)]
pub struct Block {
    pub name: &'static str,
    pub parent: usize,
    pub height: u8,
    pub payloads: Vec<Inclusion>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scenario {
    /// Transactions produce some, all, or a superset of a prepared attempt's effects.
    Effects,
    /// A dependent action runs before finality while chain views can change or arrive late.
    Reorg,
    /// An attempt's grounding expires without its effects appearing on the chain.
    Expiry,
}

#[derive(Clone, Debug)]
pub struct LifecycleModel {
    pub scenario: Scenario,
    blocks: Vec<Block>,
    pub max_grounding_age: u8,
    pub fault: Fault,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Ledger {
    pub effects: Effects,
    pub accepted_local: AttemptSet,
    pub rejected_local: AttemptSet,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Observation {
    pub revision: u8,
    pub head: usize,
    pub finalized: usize,
}

/// What the orchestrator and store hold for one local attempt.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct AttemptState {
    /// Grounding block of the payload the orchestrator saved.
    pub journal: Option<usize>,
    /// Grounding block bound by the preparation acknowledgement the orchestrator received.
    pub acknowledgement: Option<usize>,
    pub submitted: bool,
    pub preparation: Option<Preparation>,
    pub coverage: Coverage,
    pub was_observed: bool,
}

impl AttemptState {
    pub fn prepared(&self) -> bool {
        self.preparation.is_some()
    }

    pub fn protected(&self) -> bool {
        self.preparation
            .is_some_and(|preparation| preparation.protected)
    }
}

/// The store's durable record of a prepared attempt.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Preparation {
    pub grounding: usize,
    pub protected: bool,
}

#[derive(Clone, Copy, Default, Eq, Hash, PartialEq)]
pub struct Attempts([AttemptState; 3]);

impl Attempts {
    pub fn iter(&self) -> impl Iterator<Item = (Attempt, &AttemptState)> {
        Attempt::ALL.into_iter().zip(&self.0)
    }
}

impl Index<Attempt> for Attempts {
    type Output = AttemptState;

    fn index(&self, attempt: Attempt) -> &AttemptState {
        &self.0[attempt as usize]
    }
}

impl IndexMut<Attempt> for Attempts {
    fn index_mut(&mut self, attempt: Attempt) -> &mut AttemptState {
        &mut self.0[attempt as usize]
    }
}

impl fmt::Debug for Attempts {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.debug_map().entries(self.iter()).finish()
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct LifecycleState {
    pub chain_head: usize,
    pub chain_finalized: usize,
    pub reorg_used: bool,
    pub ledgers: Vec<Option<Ledger>>,
    pub synchronizer: Observation,
    pub observations: Vec<Observation>,
    pub store_view: Observation,
    pub highest_revision_received: u8,
    pub attempts: Attempts,
    pub contents: Objects,
    pub available: Objects,
    pub deletion_eligible: Objects,
    pub observation_reversed: bool,
    /// Set when a block accepts a prepared attempt whose protection the store had
    /// already released, judged by the chain rather than by the resolution rule.
    pub released_attempt_applied: bool,
}

impl LifecycleState {
    pub fn ledger(&self, block: usize) -> Ledger {
        self.ledgers[block].expect("referenced block has been materialized")
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum LifecycleStep {
    /// The orchestrator saves a proof for a later attempt, grounded at the synchronizer
    /// head and consuming inputs the store reports available.
    JournalProof(Attempt),
    /// The store durably records the attempt and its outputs, protecting its inputs.
    Prepare(Attempt),
    /// The orchestrator receives authorization for an already durable preparation.
    ReceivePreparation(Attempt),
    /// The orchestrator submits the saved payload whose grounding matches its authorization.
    Submit(Attempt),
    /// A new block becomes the chain tip; its payloads are accepted or rejected in order.
    Extend(usize),
    /// The chain rolls back to an ancestor before extending along a replacement branch.
    ReorgTo(usize),
    /// A canonical block becomes finalized, although the store may not yet know this.
    Finalize(usize),
    /// The synchronizer follows chain progress or rollback and publishes its resulting view.
    ProcessSynchronizer,
    /// The store receives a published snapshot, possibly delayed or duplicated, and
    /// reconciles its objects and preparations if the snapshot is newer than its view.
    DeliverObservation(usize),
}

impl LifecycleModel {
    pub fn new(scenario: Scenario) -> Self {
        use Attempt::{Dependent, Parent, Replacement};
        use Inclusion::{External, Local};
        let mut model = Self {
            scenario,
            blocks: vec![Block {
                name: "genesis",
                parent: GENESIS,
                height: 0,
                payloads: vec![],
            }],
            max_grounding_age: 1,
            fault: Fault::None,
        };
        match scenario {
            Scenario::Effects => {
                model.add_block("parent", GENESIS, vec![Local(Parent)]);
                for (name, inputs, created) in [
                    ("partial-input-conflict", Objects::X, Objects::R),
                    ("creation-conflict", Objects::Z, Objects::P),
                    (
                        "all-inputs-conflict",
                        Objects::X.union(Objects::Y),
                        Objects::R,
                    ),
                    ("superset", Objects::INITIAL, Objects::P.union(Objects::R)),
                ] {
                    model.add_block(
                        name,
                        GENESIS,
                        vec![External(Transaction::consuming(inputs, created))],
                    );
                }
                model.add_block(
                    "superset-consumes-output",
                    GENESIS,
                    vec![External(Transaction {
                        inputs: Objects::INITIAL,
                        effects: Effects {
                            created: Objects::P.union(Objects::Q),
                            nullifiers: Objects::INITIAL.union(Objects::P),
                        },
                    })],
                );
            }
            Scenario::Reorg => {
                let parent = model.add_block("parent", GENESIS, vec![Local(Parent)]);
                model.add_block("dependent", parent, vec![Local(Dependent)]);
                let competitor = model.add_block(
                    "competitor",
                    GENESIS,
                    vec![External(Transaction::consuming(Objects::X, Objects::R))],
                );
                model.add_block(
                    "dependent-on-replacement-branch",
                    competitor,
                    vec![Local(Dependent)],
                );
            }
            Scenario::Expiry => {
                let empty = model.add_block("empty", GENESIS, vec![]);
                let expired = model.add_block("expired-parent", empty, vec![Local(Parent)]);
                model.add_block("replacement", expired, vec![Local(Replacement)]);
            }
        }
        model
    }

    /// Adds a block on top of `parent` and returns its index.
    pub fn add_block(
        &mut self,
        name: &'static str,
        parent: usize,
        payloads: Vec<Inclusion>,
    ) -> usize {
        let block = self.blocks.len();
        self.blocks.push(Block {
            name,
            parent,
            height: self.blocks[parent].height + 1,
            payloads,
        });
        block
    }

    pub fn block(&self, name: &str) -> usize {
        self.blocks
            .iter()
            .position(|block| block.name == name)
            .unwrap_or_else(|| panic!("no block named {name:?}"))
    }

    /// The attempt beyond the initial parent that this scenario journals and prepares.
    fn later_attempt(&self) -> Option<Attempt> {
        match self.scenario {
            Scenario::Effects => None,
            Scenario::Reorg => Some(Attempt::Dependent),
            Scenario::Expiry => Some(Attempt::Replacement),
        }
    }

    fn authorized(&self, state: &LifecycleState, attempt: Attempt) -> bool {
        let Some(grounding) = state.attempts[attempt].journal else {
            return false;
        };
        if self.fault == Fault::AuthorizeReplacementWithOldAck {
            // The faulty orchestrator identifies an attempt by its effects, ignoring grounding.
            state.attempts.iter().any(|(other, other_state)| {
                other_state.acknowledgement.is_some()
                    && other.transaction().effects == attempt.transaction().effects
            })
        } else {
            state.attempts[attempt].acknowledgement == Some(grounding)
        }
    }

    pub fn is_ancestor(&self, ancestor: usize, mut descendant: usize) -> bool {
        while descendant != ancestor && descendant != 0 {
            descendant = self.blocks[descendant].parent;
        }
        descendant == ancestor
    }

    fn common_ancestor(&self, mut left: usize, right: usize) -> usize {
        while !self.is_ancestor(left, right) {
            left = self.blocks[left].parent;
        }
        left
    }

    pub fn apply_payload(
        &self,
        state: &LifecycleState,
        block: usize,
        grounding: usize,
        transaction: Transaction,
        effects: Effects,
    ) -> Option<Effects> {
        if !self.is_ancestor(grounding, self.blocks[block].parent)
            || self.blocks[block].height - self.blocks[grounding].height > self.max_grounding_age
        {
            return None;
        }
        // This represents input membership established by a valid proof.
        if !state
            .ledger(grounding)
            .effects
            .created
            .contains(transaction.inputs)
        {
            return None;
        }
        transaction.effects.apply(effects)
    }

    fn derive_block(&self, state: &LifecycleState, block: usize) -> Option<Ledger> {
        let mut ledger = state.ledger(self.blocks[block].parent);
        for inclusion in &self.blocks[block].payloads {
            let (transaction, grounding, local_attempt) = match inclusion {
                Inclusion::Local(attempt) => {
                    let attempt_state = &state.attempts[*attempt];
                    if !attempt_state.submitted {
                        return None;
                    }
                    (
                        attempt.transaction(),
                        attempt_state.journal?,
                        Some(*attempt),
                    )
                }
                Inclusion::External(transaction) => (*transaction, self.blocks[block].parent, None),
            };
            if let Some(effects) =
                self.apply_payload(state, block, grounding, transaction, ledger.effects)
            {
                ledger.effects = effects;
                if let Some(attempt) = local_attempt {
                    ledger.accepted_local.insert(attempt);
                }
            } else if let Some(attempt) = local_attempt {
                ledger.rejected_local.insert(attempt);
            }
        }
        Some(ledger)
    }

    pub fn permanently_inapplicable(&self, state: &LifecycleState, attempt: Attempt) -> bool {
        let Some(Preparation { grounding, .. }) = state.attempts[attempt].preparation else {
            return false;
        };
        let finalized = state.store_view.finalized;
        let expiry_reference = if self.fault == Fault::ExpireOnObservedHead {
            state.store_view.head
        } else {
            finalized
        };
        attempt
            .transaction()
            .effects
            .collides(state.ledger(finalized).effects)
            || (!self.is_ancestor(grounding, finalized) && !self.is_ancestor(finalized, grounding))
            || self.blocks[expiry_reference]
                .height
                .saturating_sub(self.blocks[grounding].height)
                >= self.max_grounding_age
    }

    fn reconcile(&self, state: &mut LifecycleState) {
        let observed = state.ledger(state.store_view.head).effects;
        let finalized = state.ledger(state.store_view.finalized).effects;
        let mut protected_inputs = Objects::EMPTY;
        for attempt in Attempt::ALL {
            let Some(preparation) = state.attempts[attempt].preparation else {
                continue;
            };
            let transaction = attempt.transaction();
            let coverage = transaction.effects.coverage(observed);
            let mut protected = !self.permanently_inapplicable(state, attempt);
            if self.fault == Fault::ReleaseOnObservation && coverage != Coverage::None {
                protected = false;
            }
            let attempt_state = &mut state.attempts[attempt];
            attempt_state.coverage = coverage;
            state.observation_reversed |= attempt_state.was_observed && coverage != Coverage::Full;
            attempt_state.was_observed |= coverage == Coverage::Full;
            attempt_state.preparation = Some(Preparation {
                protected,
                ..preparation
            });
            if protected {
                protected_inputs = protected_inputs.union(transaction.inputs);
            }
        }
        let mut created = observed.created;
        if self.fault == Fault::KeepOrphanOutputsAvailable {
            created = created.union(state.available);
        }
        state.available = state
            .contents
            .intersection(created)
            .without(observed.nullifiers)
            .without(protected_inputs);
        state.deletion_eligible =
            state
                .contents
                .intersection(if self.fault == Fault::CleanupOnObservation {
                    observed.nullifiers
                } else {
                    finalized.nullifiers
                });
    }
}

impl Model for LifecycleModel {
    type State = LifecycleState;
    type Action = LifecycleStep;

    fn init_states(&self) -> Vec<Self::State> {
        let mut ledgers = vec![None; self.blocks.len()];
        ledgers[GENESIS] = Some(Ledger {
            effects: Effects {
                created: Objects::INITIAL,
                nullifiers: Objects::EMPTY,
            },
            accepted_local: AttemptSet::default(),
            rejected_local: AttemptSet::default(),
        });
        let mut attempts = Attempts::default();
        attempts[Attempt::Parent] = AttemptState {
            journal: Some(GENESIS),
            preparation: Some(Preparation {
                grounding: GENESIS,
                protected: true,
            }),
            ..AttemptState::default()
        };
        vec![LifecycleState {
            chain_head: GENESIS,
            chain_finalized: GENESIS,
            reorg_used: false,
            ledgers,
            synchronizer: Observation::default(),
            observations: vec![],
            store_view: Observation::default(),
            highest_revision_received: 0,
            attempts,
            contents: Objects::INITIAL.union(Objects::P),
            available: Objects::Z,
            deletion_eligible: Objects::EMPTY,
            observation_reversed: false,
            released_attempt_applied: false,
        }]
    }

    fn actions(&self, state: &Self::State, actions: &mut Vec<Self::Action>) {
        use LifecycleStep::*;
        actions.extend([
            ReceivePreparation(Attempt::Parent),
            Submit(Attempt::Parent),
            ProcessSynchronizer,
        ]);
        if let Some(attempt) = self.later_attempt() {
            actions.extend([
                JournalProof(attempt),
                Prepare(attempt),
                ReceivePreparation(attempt),
                Submit(attempt),
            ]);
        }
        for block in 1..self.blocks.len() {
            actions.extend([Extend(block), Finalize(block)]);
        }
        if self.scenario == Scenario::Reorg && !state.reorg_used {
            for block in 0..self.blocks.len() {
                actions.push(ReorgTo(block));
            }
        }
        for observation in 0..state.observations.len() {
            actions.push(DeliverObservation(observation));
        }
    }

    fn next_state(&self, previous: &Self::State, action: Self::Action) -> Option<Self::State> {
        use LifecycleStep::*;
        let mut state = previous.clone();
        match action {
            JournalProof(attempt)
                if state.attempts[attempt].journal.is_none()
                    && state.available.contains(attempt.transaction().inputs)
                    && state
                        .ledger(state.synchronizer.head)
                        .effects
                        .created
                        .contains(attempt.transaction().inputs) =>
            {
                state.attempts[attempt].journal = Some(state.synchronizer.head);
            }
            Prepare(attempt)
                if state.attempts[attempt].journal.is_some()
                    && !state.attempts[attempt].prepared()
                    && state.available.contains(attempt.transaction().inputs) =>
            {
                let attempt_state = &mut state.attempts[attempt];
                attempt_state.preparation = attempt_state.journal.map(|grounding| Preparation {
                    grounding,
                    protected: true,
                });
                state.contents = state.contents.union(attempt.transaction().effects.created);
                self.reconcile(&mut state);
            }
            ReceivePreparation(attempt) if state.attempts[attempt].prepared() => {
                let attempt_state = &mut state.attempts[attempt];
                attempt_state.acknowledgement = attempt_state
                    .preparation
                    .map(|preparation| preparation.grounding);
            }
            Submit(attempt) if self.authorized(&state, attempt) => {
                state.attempts[attempt].submitted = true;
            }
            Extend(block) if self.blocks[block].parent == state.chain_head => {
                let ledger = self.derive_block(&state, block)?;
                let previously_accepted = state.ledger(state.chain_head).accepted_local;
                state.released_attempt_applied |=
                    state.attempts.iter().any(|(attempt, attempt_state)| {
                        ledger.accepted_local.contains(attempt)
                            && !previously_accepted.contains(attempt)
                            && attempt_state.prepared()
                            && !attempt_state.protected()
                    });
                state.ledgers[block] = Some(ledger);
                state.chain_head = block;
            }
            ReorgTo(block)
                if self.scenario == Scenario::Reorg
                    && !state.reorg_used
                    && block != state.chain_head
                    && self.is_ancestor(block, state.chain_head)
                    && self.is_ancestor(state.chain_finalized, block) =>
            {
                state.reorg_used = true;
                state.chain_head = block;
            }
            Finalize(block)
                if block != state.chain_finalized
                    && self.is_ancestor(state.chain_finalized, block)
                    && self.is_ancestor(block, state.chain_head) =>
            {
                state.chain_finalized = block;
            }
            ProcessSynchronizer => {
                let mut next_head = state.synchronizer.head;
                if !self.is_ancestor(next_head, state.chain_head) {
                    next_head = self.common_ancestor(next_head, state.chain_head);
                } else if next_head != state.chain_head {
                    next_head = state.chain_head;
                    while self.blocks[next_head].parent != state.synchronizer.head {
                        next_head = self.blocks[next_head].parent;
                    }
                }
                let next_finalized = self.common_ancestor(next_head, state.chain_finalized);
                if next_head == state.synchronizer.head
                    && next_finalized == state.synchronizer.finalized
                {
                    return None;
                }
                let observation = Observation {
                    revision: state.synchronizer.revision + 1,
                    head: next_head,
                    finalized: next_finalized,
                };
                state.synchronizer = observation;
                state.observations.push(observation);
            }
            DeliverObservation(index) => {
                let observation = state.observations[index];
                state.highest_revision_received =
                    state.highest_revision_received.max(observation.revision);
                if observation.revision > state.store_view.revision
                    || self.fault == Fault::AcceptOldObservations
                {
                    state.store_view = observation;
                    self.reconcile(&mut state);
                }
            }
            _ => return None,
        }
        (state != *previous).then_some(state)
    }

    fn format_action(&self, action: &Self::Action) -> String {
        match action {
            LifecycleStep::Extend(block) => format!("Extend({})", self.blocks[*block].name),
            LifecycleStep::ReorgTo(block) => format!("ReorgTo({})", self.blocks[*block].name),
            LifecycleStep::Finalize(block) => format!("Finalize({})", self.blocks[*block].name),
            _ => format!("{action:?}"),
        }
    }

    fn properties(&self) -> Vec<Property<Self>> {
        let mut properties = vec![
            Property::<Self>::always(
                "submitted payloads have matching preparations",
                |_, state| {
                    state.attempts.iter().all(|(_, attempt_state)| {
                        !attempt_state.submitted
                            || attempt_state.preparation.is_some_and(|preparation| {
                                Some(preparation.grounding) == attempt_state.journal
                            })
                    })
                },
            ),
            Property::<Self>::always("released attempts cannot apply", |_, state| {
                !state.released_attempt_applied
            }),
            Property::<Self>::always(
                "availability follows the applied observation",
                |_, state| {
                    let observed = state.ledger(state.store_view.head).effects;
                    let protected_inputs = state
                        .attempts
                        .iter()
                        .filter(|(_, attempt_state)| attempt_state.protected())
                        .fold(Objects::EMPTY, |inputs, (attempt, _)| {
                            inputs.union(attempt.transaction().inputs)
                        });
                    state
                        .contents
                        .intersection(observed.created)
                        .without(observed.nullifiers)
                        .without(protected_inputs)
                        .contains(state.available)
                },
            ),
            Property::<Self>::always("old observations cannot replace newer ones", |_, state| {
                state.store_view.revision == state.highest_revision_received
            }),
            Property::<Self>::always("cleanup requires finalized consumption", |_, state| {
                state
                    .ledger(state.chain_finalized)
                    .effects
                    .nullifiers
                    .contains(state.deletion_eligible)
            }),
            Property::<Self>::always("prepared output contents are retained", |_, state| {
                state.attempts.iter().all(|(attempt, attempt_state)| {
                    !attempt_state.prepared()
                        || state
                            .contents
                            .contains(attempt.transaction().effects.created)
                })
            }),
        ];
        match self.scenario {
            Scenario::Effects => properties.extend([
                Property::<Self>::sometimes("partial effects are reachable", |_, state| {
                    state.attempts[Attempt::Parent].coverage == Coverage::Partial
                }),
                Property::<Self>::sometimes("external superset satisfies intent", |_, state| {
                    state.attempts[Attempt::Parent].coverage == Coverage::Full
                        && !state
                            .ledger(state.store_view.head)
                            .accepted_local
                            .contains(Attempt::Parent)
                }),
                Property::<Self>::sometimes("satisfied output may already be spent", |_, state| {
                    state.attempts[Attempt::Parent].coverage == Coverage::Full
                        && state
                            .ledger(state.store_view.head)
                            .effects
                            .nullifiers
                            .contains(Objects::P)
                        && !state.available.overlaps(Objects::P)
                }),
                Property::<Self>::sometimes("finalized effects release protection", |_, state| {
                    state.attempts[Attempt::Parent].coverage == Coverage::Full
                        && !state.attempts[Attempt::Parent].protected()
                }),
            ]),
            Scenario::Reorg => properties.extend([
                Property::<Self>::sometimes("dependent execution before finality", |_, state| {
                    state
                        .ledger(state.chain_head)
                        .accepted_local
                        .contains(Attempt::Dependent)
                        && state.chain_finalized == GENESIS
                }),
                Property::<Self>::sometimes("observed effects can be reversed", |_, state| {
                    state.observation_reversed
                }),
                Property::<Self>::sometimes(
                    "orphan grounding rejects dependent transaction",
                    |model, state| {
                        state
                            .ledger(state.chain_head)
                            .rejected_local
                            .contains(Attempt::Dependent)
                            && state.attempts[Attempt::Dependent]
                                .journal
                                .is_some_and(|root| !model.is_ancestor(root, state.chain_head))
                    },
                ),
            ]),
            Scenario::Expiry => properties.extend([
                Property::<Self>::sometimes("expired proof is rejected", |_, state| {
                    state
                        .ledger(state.chain_head)
                        .rejected_local
                        .contains(Attempt::Parent)
                }),
                Property::<Self>::sometimes(
                    "finalized expiry releases unresolved intent",
                    |_, state| {
                        state.attempts[Attempt::Parent].coverage == Coverage::None
                            && !state.attempts[Attempt::Parent].protected()
                    },
                ),
                Property::<Self>::sometimes("prepared replacement applies", |_, state| {
                    state
                        .ledger(state.chain_head)
                        .accepted_local
                        .contains(Attempt::Replacement)
                }),
            ]),
        }
        properties
    }
}
