//! The authored artifact of a jointly-assembled transaction, from
//! which every negotiation view derives.
//!
//! A [`JointTransaction`] holds what the parties agree on: who
//! participates, who finalizes, which input states enter and who holds
//! each, and the labeled event list. The labels are what a [`TxPlan`]
//! does not carry: each leaf event's performer, and what the event
//! does to key custody. Everything downstream is a projection of the
//! one value: [`plan`](JointTransaction::plan) erases the labels,
//! [`graph`](JointTransaction::graph) derives the proving schedule
//! from who performs what, and
//! [`final_custody`](JointTransaction::final_custody) maps each
//! surviving state to its one owner.
//!
//! Construction runs all validation and derives every view, so a
//! `JointTransaction` that exists is buildable and custody-sound and
//! the projections are pure reads on it. The custody invariant: no
//! state in the final live set has its key known by more than one
//! party. Within the protocol's own messages knowledge of a key only
//! grows, and the only thing that narrows custody is a fresh-key
//! event, so the check is a syntactic fold over the events, no
//! epistemic reasoning. The constructor rejects only the unbuildable
//! and the custody-unsound, never what a party merely should not
//! want: intent stays out of the shared artifact and belongs to each
//! client's accept decision.

use std::collections::HashMap;

use pod2::middleware::{Hash, Value};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::graph::{NodeKind, StatementGraph, StatementNode};
use crate::plan::{PlannedEvent, TxPlan};

/// What a class-action mutation did to the affected object's key.
///
/// Only mutations carry this: a transfer resets custody to its
/// receiver and a mint to its minter by construction. The SDK owns the
/// judgment of which value a named class action's key write earns;
/// here the three values are a closed vocabulary. `Unknown` poisons
/// custody, so it cannot be a surviving state's last key event; the
/// in-plan repair is appending a transfer to self.
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

/// One labeled event. References object states by commitment only,
/// exactly like [`PlannedEvent`], plus the party labels the custody
/// fold and the graph derivation consume. Each leaf affects a single
/// object and is performed by a party holding that object's full
/// state; `Transfer` is the one split event, erasure by the sender and
/// key binding by the receiver, which is why it carries two parties
/// and no annotation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum JointEvent {
    /// A mint under the minter's own fresh key, disclosed to no one.
    /// A disclosed mint (full dict handed to a recipient) is a later
    /// vocabulary extension.
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
    /// A transfer wrapped as its own top-level action, the only shape
    /// the constructor accepts for one.
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

    /// Classify one top-level action for the two consumers that must
    /// agree on the distinction, the custody fold and the graph
    /// derivation. A new crossing kind added here forces both matches
    /// to update instead of falling through one of them silently.
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

/// One input state: its commitment and the participant holding it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputState {
    pub commitment: Hash,
    pub holder: String,
}

/// A jointly-assembled transaction as one authored value, with all
/// derived views computed at construction.
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

/// Equality is over the authored fields only: the derived views are
/// functions of them, so comparing two independently constructed
/// documents compares the whole deal, labels included.
impl PartialEq for JointTransaction {
    fn eq(&self, other: &Self) -> bool {
        self.participants == other.participants
            && self.finalizer == other.finalizer
            && self.inputs == other.inputs
            && self.events == other.events
    }
}

impl Eq for JointTransaction {}

/// Who can open a state as the events unfold. Feasibility forces
/// every event's performer to hold what it touches, so sole custody
/// coincides with holding and the only custody fact beyond the holder
/// is whether an unclassified key write poisoned the lineage.
struct StateInfo {
    holder: String,
    key_unclassified: bool,
}

impl JointTransaction {
    /// Validate and build. Structure is delegated to `TxPlan::new`
    /// on the label-erased events; on top of it this rejects
    /// assignments the machinery cannot execute (a performer not
    /// holding what its event touches, non-transfer events by a
    /// non-finalizer, a transfer sharing an action with other events)
    /// and transactions whose final live set contains a state with
    /// custody other than exactly one party.
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

    /// The label-erased projection: what every party must derive
    /// identically before proving starts.
    pub fn plan(&self) -> &TxPlan {
        &self.plan
    }

    /// Each surviving state's one owner. The constructor made any
    /// other custody unrepresentable.
    pub fn final_custody(&self) -> &HashMap<Hash, String> {
        &self.final_custody
    }

    /// The statement graph, derived instead of hand-declared. Per
    /// transfer: an offer node by the sender, plus an acceptance node
    /// fed by it when the receiver is not the finalizer; nothing for
    /// actions internal to the finalizer; a finalize node fed by the
    /// last node of every leg that crosses. Node ids come from the
    /// top-level action index.
    pub fn graph(&self) -> &StatementGraph {
        &self.graph
    }
}

/// Every party an event names must be a participant. Checked once
/// up-front, so the custody fold carries only custody semantics and
/// no membership check depends on the finalizer restriction below.
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

/// Walk a finalizer-internal action's contents, applying each leaf.
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
                // Recording an insert requires holding the dict, and a
                // non-finalizer's action statements have no
                // contribution type to cross in; both machinery gaps,
                // not invariants, so this loosens when they close.
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

/// Update the holder/custody map for one leaf event.
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

/// Take a consumed state out of the map, requiring the consuming
/// party to hold it.
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

/// Erase the party labels, leaving the plan's vocabulary. A transfer
/// is a mutate to the plan: the split into erasure and key binding
/// matters to provers, not to positions or sets.
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

/// Derive the statement graph from the classified top-level actions.
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
