//! Pure derivation of a transaction's negotiated quantities from its
//! planned event sequence.
//!
//! A [`TxPlan`] holds only commitments plus the nullifiers their owners
//! contribute: no dicts, no keys, no statements, no state header. From
//! those it derives everything the parties to a jointly-assembled
//! transaction must compute identically before proving starts: the
//! chain position of every event, the scope each event's guard is
//! dispatched against, the final live and nullifier sets, `tx_final`,
//! and the per-header context. Being header-free, a plan survives
//! re-grounding; only [`TxPlan::context`] brings the header in.
//!
//! The derivation mirrors the fold `TxBuilder`'s recorders and
//! `finalize` perform. It is a second implementation on purpose (a
//! negotiating party has no builder), and the agreement between the
//! two is pinned by tests.

use pod2::middleware::{Hash, Value, containers::Set};
use pod2utils::set;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{
    chain_seed, chain_step, context_commitment, event_hash_delete, event_hash_insert,
    event_hash_mutate, top_level_tx,
};

/// One planned event, referencing object states by commitment only.
/// `Mutate` and `Delete` also carry the consumed state's nullifier, the
/// one value in a plan that only that state's owner can derive:
/// collecting them is what completes the plan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PlannedEvent {
    Insert {
        new: Hash,
    },
    Mutate {
        old: Hash,
        new: Hash,
        nullifier: Hash,
    },
    Delete {
        old: Hash,
        nullifier: Hash,
    },
    Action(Vec<PlannedEvent>),
}

/// Derived positions for one leaf (non-action) event: its own chain
/// step, and the chain range of its innermost enclosing action.
#[derive(Clone, Debug)]
struct PlannedLeaf {
    prev_chain: Hash,
    chain_after: Hash,
    scope_start: Hash,
    scope_end: Hash,
}

/// A transaction's planned effect and every quantity derived from it.
///
/// Construction runs the full fold, so a `TxPlan` that exists is
/// structurally valid and all accessors are pure reads. Leaf accessors
/// index the non-action events in depth-first order.
#[derive(Clone, Debug)]
pub struct TxPlan {
    inputs: Vec<Hash>,
    events: Vec<PlannedEvent>,
    chain_start: Hash,
    chain_end: Hash,
    leaves: Vec<PlannedLeaf>,
    live: Set,
    nullifiers: Set,
    tx_final: Hash,
}

impl TxPlan {
    /// Derive every negotiated quantity from `inputs` and `events`,
    /// validating the same structure `TxBuilder` enforces at record
    /// time: top-level events are actions, actions are non-empty,
    /// consumed states are live, created states and nullifiers are
    /// fresh.
    pub fn new(inputs: Vec<Hash>, events: Vec<PlannedEvent>) -> anyhow::Result<Self> {
        anyhow::ensure!(!events.is_empty(), "plan must contain at least one action");
        let mut inputs_set = set!();
        for input in &inputs {
            insert_fresh(&mut inputs_set, *input, "input commitment")?;
        }
        let chain_start = chain_seed(&inputs_set);

        let mut fold = Fold {
            live: inputs_set,
            nullifiers: set!(),
            leaves: Vec::new(),
        };
        let mut chain = chain_start;
        for event in &events {
            let PlannedEvent::Action(contents) = event else {
                anyhow::bail!("top-level plan event must be an action");
            };
            chain = fold.action(contents, chain)?;
        }

        let tx_final = top_level_tx(&fold.live, &fold.nullifiers).commitment();
        Ok(Self {
            inputs,
            events,
            chain_start,
            chain_end: chain,
            leaves: fold.leaves,
            live: fold.live,
            nullifiers: fold.nullifiers,
            tx_final,
        })
    }

    pub fn inputs(&self) -> &[Hash] {
        &self.inputs
    }

    pub fn events(&self) -> &[PlannedEvent] {
        &self.events
    }

    /// `H(inputs, {})`: the chain position before the first event.
    pub fn chain_start(&self) -> Hash {
        self.chain_start
    }

    /// The chain position after the last event.
    pub fn chain_end(&self) -> Hash {
        self.chain_end
    }

    /// Number of leaf (non-action) events. Leaf accessors index them in
    /// depth-first order.
    pub fn leaf_count(&self) -> usize {
        self.leaves.len()
    }

    /// The chain step of leaf `index`: the positions its
    /// `TxInsert`/`TxMutate`/`TxDelete` statement is proven against.
    pub fn event_range(&self, index: usize) -> (Hash, Hash) {
        let leaf = self.leaves.get(index).expect("leaf index out of range");
        (leaf.prev_chain, leaf.chain_after)
    }

    /// The chain range of leaf `index`'s innermost enclosing action:
    /// the scope its guard evidence is dispatched against.
    pub fn scope(&self, index: usize) -> (Hash, Hash) {
        let leaf = self.leaves.get(index).expect("leaf index out of range");
        (leaf.scope_start, leaf.scope_end)
    }

    /// Object commitments left live by the transaction.
    pub fn live(&self) -> &Set {
        &self.live
    }

    /// Nullifiers the transaction emits.
    pub fn nullifiers(&self) -> &Set {
        &self.nullifiers
    }

    /// Commitment of the final transaction dict: the value the relayer
    /// publishes and every spend endorsement binds.
    pub fn tx_final(&self) -> Hash {
        self.tx_final
    }

    /// The context every spend endorses when the transaction grounds
    /// against the state header committing to `state_root`. The one
    /// derivation that brings the header in: everything else in the
    /// plan survives re-grounding, this value expires with the header.
    pub fn context(&self, state_root: Hash) -> Hash {
        context_commitment(state_root, self.tx_final)
    }
}

/// Working state of the derivation walk.
struct Fold {
    live: Set,
    nullifiers: Set,
    leaves: Vec<PlannedLeaf>,
}

impl Fold {
    /// Walk one action's contents from chain position `scope_start`,
    /// returning the position after its last event. The chain folds
    /// flat through nested actions; an action only bounds the scope its
    /// direct leaves' guards see, so those scopes are assigned once the
    /// end position is known.
    fn action(&mut self, contents: &[PlannedEvent], scope_start: Hash) -> anyhow::Result<Hash> {
        anyhow::ensure!(
            !contents.is_empty(),
            "plan action must contain at least one event"
        );
        let mut chain = scope_start;
        let mut direct_leaves = Vec::new();
        for event in contents {
            if let PlannedEvent::Action(inner) = event {
                chain = self.action(inner, chain)?;
                continue;
            }
            let event_hash = self.apply_leaf(event)?;
            let prev_chain = chain;
            chain = chain_step(prev_chain, event_hash);
            direct_leaves.push(self.leaves.len());
            self.leaves.push(PlannedLeaf {
                prev_chain,
                chain_after: chain,
                scope_start,
                scope_end: chain,
            });
        }
        for index in direct_leaves {
            self.leaves[index].scope_end = chain;
        }
        Ok(chain)
    }

    /// Update the live/nullifier sets for one leaf event and return its
    /// event hash.
    fn apply_leaf(&mut self, event: &PlannedEvent) -> anyhow::Result<Hash> {
        Ok(match event {
            PlannedEvent::Insert { new } => {
                insert_fresh(&mut self.live, *new, "created state")?;
                event_hash_insert(Value::from(*new))
            }
            PlannedEvent::Mutate {
                old,
                new,
                nullifier,
            } => {
                delete_live(&mut self.live, *old)?;
                insert_fresh(&mut self.live, *new, "created state")?;
                insert_fresh(&mut self.nullifiers, *nullifier, "nullifier")?;
                event_hash_mutate(Value::from(*old), Value::from(*new))
            }
            PlannedEvent::Delete { old, nullifier } => {
                delete_live(&mut self.live, *old)?;
                insert_fresh(&mut self.nullifiers, *nullifier, "nullifier")?;
                event_hash_delete(Value::from(*old))
            }
            PlannedEvent::Action(_) => unreachable!("apply_leaf is never called on actions"),
        })
    }
}

fn insert_fresh(set: &mut Set, value: Hash, what: &str) -> anyhow::Result<()> {
    let value = Value::from(value);
    anyhow::ensure!(!set.contains(&value)?, "duplicate {what}: {value}");
    set.insert(&value)?;
    Ok(())
}

fn delete_live(live: &mut Set, old: Hash) -> anyhow::Result<()> {
    let value = Value::from(old);
    anyhow::ensure!(
        live.contains(&value)?,
        "consumed state is not live: {value}"
    );
    live.delete(&value)?;
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TxPlanSerde {
    inputs: Vec<Hash>,
    events: Vec<PlannedEvent>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TxPlanSerdeRef<'a> {
    inputs: &'a [Hash],
    events: &'a [PlannedEvent],
}

impl Serialize for TxPlan {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        TxPlanSerdeRef {
            inputs: &self.inputs,
            events: &self.events,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for TxPlan {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let payload = TxPlanSerde::deserialize(deserializer)?;
        TxPlan::new(payload.inputs, payload.events).map_err(serde::de::Error::custom)
    }
}
