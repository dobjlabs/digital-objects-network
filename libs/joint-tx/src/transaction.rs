//! Authored agreement for a jointly assembled transaction.
//!
//! [`JointTransaction`] records participants, the finalizer, input
//! custody, and labeled events. It derives the label-free
//! [`plan`](JointTransaction::plan), proving
//! [`graph`](JointTransaction::graph), and
//! [`final_custody`](JointTransaction::final_custody).
//!
//! Construction validates feasibility and folds key custody through the
//! events. Every surviving state must end with one holder and no
//! unclassified key write. Policy and participant intent remain client-side.

use std::collections::HashMap;

use pod2::middleware::{Hash, Value};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::graph::{NodeKind, StatementGraph, StatementNode};
use crate::plan::{PlannedEvent, TxPlan};

/// How a mutation changed the affected state's key.
///
/// `Unknown` marks custody unclassified until a later fresh-key event,
/// such as a transfer to self, restores a single holder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KeyAnnotation {
    /// The performer wrote a key it freshly generated.
    Fresh,
    /// No key write: the state keeps its predecessor's key.
    Inherited,
    /// A key write that could not be classified as fresh.
    Unknown,
}

/// One commitment-only event with the labels needed for custody and proving.
///
/// Each ordinary leaf has one performer. `Transfer` splits key erasure and
/// key binding between sender and receiver.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum JointEvent {
    /// Mint under a fresh key held by `minter`.
    Insert {
        minter: String,
        new: Hash,
    },
    Mutate {
        performer: String,
        old: Hash,
        new: Hash,
        nullifier: Hash,
        key: KeyAnnotation,
    },
    Delete {
        performer: String,
        old: Hash,
        nullifier: Hash,
    },
    Transfer {
        sender: String,
        receiver: String,
        old: Hash,
        new: Hash,
        nullifier: Hash,
    },
    Action(Vec<JointEvent>),
}

impl JointEvent {
    /// Wrap a transfer in the required single-event top-level action.
    pub fn transfer_leg(
        sender: impl Into<String>,
        receiver: impl Into<String>,
        old: Hash,
        new: Hash,
        nullifier: Hash,
    ) -> JointEvent {
        JointEvent::Action(vec![JointEvent::Transfer {
            sender: sender.into(),
            receiver: receiver.into(),
            old,
            new,
            nullifier,
        }])
    }

    /// Classify a top-level action for custody and graph derivation.
    fn classify_top_level(&self) -> TopLevelAction<'_> {
        let JointEvent::Action(contents) = self else {
            unreachable!("top-level events are actions once the plan projection passed");
        };
        match contents.as_slice() {
            [
                leaf @ JointEvent::Transfer {
                    sender,
                    receiver,
                    old,
                    ..
                },
            ] => TopLevelAction::TransferLeg {
                sender,
                receiver,
                old: *old,
                leaf,
            },
            _ => TopLevelAction::Internal(contents),
        }
    }
}

/// One top-level action, classified.
enum TopLevelAction<'a> {
    /// Exactly one transfer: a leg that may cross parties.
    TransferLeg {
        sender: &'a str,
        receiver: &'a str,
        old: Hash,
        leaf: &'a JointEvent,
    },
    /// Anything else: every leaf must be the finalizer's own event.
    Internal(&'a [JointEvent]),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputState {
    pub commitment: Hash,
    pub holder: String,
}

/// A validated joint transaction and its derived views.
#[derive(Clone, Debug)]
pub struct JointTransaction {
    participants: Vec<String>,
    finalizer: String,
    inputs: Vec<InputState>,
    events: Vec<JointEvent>,
    plan: TxPlan,
    graph: StatementGraph,
    final_custody: HashMap<Hash, String>,
}

/// Equality is over the authored fields; the derived views are
/// functions of them.
impl PartialEq for JointTransaction {
    fn eq(&self, other: &Self) -> bool {
        self.participants == other.participants
            && self.finalizer == other.finalizer
            && self.inputs == other.inputs
            && self.events == other.events
    }
}

impl Eq for JointTransaction {}

/// Current holder and whether the latest key lineage is classified.
struct StateInfo {
    holder: String,
    key_unclassified: bool,
}

impl JointTransaction {
    /// Validate participants, plan structure, event feasibility, and final
    /// custody, then derive all views.
    pub fn new(
        participants: Vec<String>,
        finalizer: String,
        inputs: Vec<InputState>,
        events: Vec<JointEvent>,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            !participants.is_empty(),
            "a joint transaction needs at least one participant"
        );
        for (index, participant) in participants.iter().enumerate() {
            anyhow::ensure!(
                !participants[..index].contains(participant),
                "duplicate participant: {participant}"
            );
        }
        let is_participant = |party: &str| participants.iter().any(|p| p == party);
        anyhow::ensure!(
            is_participant(&finalizer),
            "finalizer {finalizer} is not a participant"
        );
        for input in &inputs {
            anyhow::ensure!(
                is_participant(&input.holder),
                "input holder {} is not a participant",
                input.holder
            );
        }
        ensure_parties_are_participants(&events, &is_participant)?;

        let plan = TxPlan::new(
            inputs.iter().map(|input| input.commitment).collect(),
            erase(&events),
        )?;

        let mut states: HashMap<Hash, StateInfo> = inputs
            .iter()
            .map(|input| {
                (
                    input.commitment,
                    StateInfo {
                        holder: input.holder.clone(),
                        key_unclassified: false,
                    },
                )
            })
            .collect();
        for event in &events {
            match event.classify_top_level() {
                TopLevelAction::TransferLeg { leaf, .. } => apply_leaf(&mut states, leaf)?,
                TopLevelAction::Internal(contents) => {
                    apply_internal(&mut states, contents, &finalizer)?
                }
            }
        }

        let mut final_custody = HashMap::new();
        for (commitment, info) in states {
            anyhow::ensure!(
                !info.key_unclassified,
                "surviving state {} has an unclassified key write as its last key event; \
                 append a transfer to repair",
                Value::from(commitment)
            );
            final_custody.insert(commitment, info.holder);
        }

        let graph = derive_graph(&finalizer, &events);
        Ok(Self {
            participants,
            finalizer,
            inputs,
            events,
            plan,
            graph,
            final_custody,
        })
    }

    pub fn participants(&self) -> &[String] {
        &self.participants
    }

    pub fn finalizer(&self) -> &str {
        &self.finalizer
    }

    pub fn inputs(&self) -> &[InputState] {
        &self.inputs
    }

    pub fn events(&self) -> &[JointEvent] {
        &self.events
    }

    pub fn plan(&self) -> &TxPlan {
        &self.plan
    }

    /// The sole holder of each surviving state.
    pub fn final_custody(&self) -> &HashMap<Hash, String> {
        &self.final_custody
    }

    pub fn graph(&self) -> &StatementGraph {
        &self.graph
    }
}

/// Validate every party label before folding custody.
fn ensure_parties_are_participants(
    events: &[JointEvent],
    is_participant: &impl Fn(&str) -> bool,
) -> anyhow::Result<()> {
    let check = |role: &str, party: &str| {
        anyhow::ensure!(is_participant(party), "{role} {party} is not a participant");
        Ok(())
    };
    for event in events {
        match event {
            JointEvent::Action(contents) => {
                ensure_parties_are_participants(contents, is_participant)?
            }
            JointEvent::Insert { minter, .. } => check("minter", minter)?,
            JointEvent::Mutate { performer, .. } | JointEvent::Delete { performer, .. } => {
                check("performer", performer)?
            }
            JointEvent::Transfer {
                sender, receiver, ..
            } => {
                check("transfer sender", sender)?;
                check("transfer receiver", receiver)?;
            }
        }
    }
    Ok(())
}

fn apply_internal(
    states: &mut HashMap<Hash, StateInfo>,
    contents: &[JointEvent],
    finalizer: &str,
) -> anyhow::Result<()> {
    for event in contents {
        match event {
            JointEvent::Action(inner) => apply_internal(states, inner, finalizer)?,
            JointEvent::Transfer { .. } => {
                anyhow::bail!("a transfer must be the sole event of its top-level action")
            }
            JointEvent::Insert {
                minter: performer, ..
            }
            | JointEvent::Mutate { performer, .. }
            | JointEvent::Delete { performer, .. } => {
                // Non-finalizer inserts and actions have no contribution type
                // yet. This restriction can loosen when those types exist.
                anyhow::ensure!(
                    performer == finalizer,
                    "event performed by {performer}: until contributed inserts and actions \
                     exist, non-transfer events must be performed by the finalizer \
                     ({finalizer})"
                );
                apply_leaf(states, event)?;
            }
        }
    }
    Ok(())
}

fn apply_leaf(states: &mut HashMap<Hash, StateInfo>, leaf: &JointEvent) -> anyhow::Result<()> {
    match leaf {
        JointEvent::Insert { minter, new } => {
            states.insert(
                *new,
                StateInfo {
                    holder: minter.clone(),
                    key_unclassified: false,
                },
            );
        }
        JointEvent::Mutate {
            performer,
            old,
            new,
            key,
            ..
        } => {
            let old_info = consume(states, old, performer, "mutate")?;
            let key_unclassified = match key {
                KeyAnnotation::Fresh => false,
                KeyAnnotation::Inherited => old_info.key_unclassified,
                KeyAnnotation::Unknown => true,
            };
            states.insert(
                *new,
                StateInfo {
                    holder: performer.clone(),
                    key_unclassified,
                },
            );
        }
        JointEvent::Delete { performer, old, .. } => {
            consume(states, old, performer, "delete")?;
        }
        JointEvent::Transfer {
            sender,
            receiver,
            old,
            new,
            ..
        } => {
            consume(states, old, sender, "transfer")?;
            states.insert(
                *new,
                StateInfo {
                    holder: receiver.clone(),
                    key_unclassified: false,
                },
            );
        }
        JointEvent::Action(_) => unreachable!("apply_leaf is never called on actions"),
    }
    Ok(())
}

/// Remove a consumed state, requiring `by` to hold it.
fn consume(
    states: &mut HashMap<Hash, StateInfo>,
    old: &Hash,
    by: &str,
    what: &str,
) -> anyhow::Result<StateInfo> {
    let Some(info) = states.remove(old) else {
        anyhow::bail!("consumed state is not live: {}", Value::from(*old));
    };
    anyhow::ensure!(
        info.holder == by,
        "{what} of {} performed by {by}, who does not hold it (holder: {})",
        Value::from(*old),
        info.holder
    );
    Ok(info)
}

/// Erase party labels, projecting each transfer to a mutation.
fn erase(events: &[JointEvent]) -> Vec<PlannedEvent> {
    events
        .iter()
        .map(|event| match event {
            JointEvent::Insert { new, .. } => PlannedEvent::Insert { new: *new },
            JointEvent::Mutate {
                old,
                new,
                nullifier,
                ..
            }
            | JointEvent::Transfer {
                old,
                new,
                nullifier,
                ..
            } => PlannedEvent::Mutate {
                old: *old,
                new: *new,
                nullifier: *nullifier,
            },
            JointEvent::Delete { old, nullifier, .. } => PlannedEvent::Delete {
                old: *old,
                nullifier: *nullifier,
            },
            JointEvent::Action(contents) => PlannedEvent::Action(erase(contents)),
        })
        .collect()
}

fn derive_graph(finalizer: &str, events: &[JointEvent]) -> StatementGraph {
    let mut nodes = Vec::new();
    let mut finalize_premises = Vec::new();
    for (index, event) in events.iter().enumerate() {
        match event.classify_top_level() {
            TopLevelAction::Internal(_) => {}
            TopLevelAction::TransferLeg {
                sender,
                receiver,
                old,
                ..
            } => {
                if sender == finalizer && receiver == finalizer {
                    continue;
                }
                let offer_id = format!("offer:{index}");
                nodes.push(StatementNode::new(
                    &offer_id,
                    sender,
                    NodeKind::TransferOffer { object: old },
                    &[],
                ));
                if receiver == finalizer {
                    finalize_premises.push(offer_id);
                } else {
                    let accept_id = format!("accept:{index}");
                    nodes.push(StatementNode::new(
                        &accept_id,
                        receiver,
                        NodeKind::TransferAcceptance { object: old },
                        &[&offer_id],
                    ));
                    finalize_premises.push(accept_id);
                }
            }
        }
    }
    nodes.push(StatementNode {
        id: "finalize".to_string(),
        producer: finalizer.to_string(),
        kind: NodeKind::Finalize,
        premises: finalize_premises,
    });
    StatementGraph::new(nodes).expect("derived graphs are well formed by construction")
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct JointTransactionSerde {
    participants: Vec<String>,
    finalizer: String,
    inputs: Vec<InputState>,
    events: Vec<JointEvent>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JointTransactionSerdeRef<'a> {
    participants: &'a [String],
    finalizer: &'a str,
    inputs: &'a [InputState],
    events: &'a [JointEvent],
}

impl Serialize for JointTransaction {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        JointTransactionSerdeRef {
            participants: &self.participants,
            finalizer: &self.finalizer,
            inputs: &self.inputs,
            events: &self.events,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for JointTransaction {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let payload = JointTransactionSerde::deserialize(deserializer)?;
        JointTransaction::new(
            payload.participants,
            payload.finalizer,
            payload.inputs,
            payload.events,
        )
        .map_err(serde::de::Error::custom)
    }
}
