//! Derive transaction quantities from an agreed event sequence.
//!
//! A [`TxPlan`] contains commitments and contributed nullifiers, but no
//! dictionaries, keys, statements, or state header. It derives event
//! positions, guard scopes, final sets, `tx_final`, and the context for a
//! selected state root.
//!
//! Plans are header-independent and survive re-grounding. Construction is
//! internal to [`crate::JointTransaction`] and mirrors `TxBuilder`'s fold.

use pod2::middleware::{Hash, Value, containers::Set};
use pod2utils::set;

use txlib::{
    chain_seed, chain_step, context_commitment, event_hash_delete, event_hash_insert,
    event_hash_mutate, top_level_tx,
};

/// One event expressed with object commitments.
/// Consuming events also carry their contributed nullifiers.
#[derive(Clone, Debug, PartialEq, Eq)]
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

/// A validated transaction effect and its derived quantities.
/// Leaf accessors use depth-first event order.
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
    /// Validate `inputs` and `events`, then derive all plan quantities.
    /// This enforces the same event structure and set transitions as
    /// `TxBuilder`.
    pub(crate) fn new(inputs: Vec<Hash>, events: Vec<PlannedEvent>) -> anyhow::Result<Self> {
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

    /// Transaction context for `state_root`. Only this derived value changes
    /// when the plan is re-grounded.
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
    /// Fold one action from `scope_start` and return its end position.
    /// Nested actions share the flat chain but define separate guard scopes.
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
