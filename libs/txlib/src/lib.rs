//! Transaction predicates for verifiable state transitions.
//!
//! A transaction consumes grounded input objects, emits a sequence of
//! insert/mutate/delete events grouped into actions, and produces a
//! `TxFinalized` proof. The event sequence is recorded as a hash chain
//! and verified by replay at finalize time; only the state root, final
//! tx commitment, and nullifier set are public.
//!
//! # API layering
//!
//! The public surface is intentionally small:
//!
//! - [`TxBuilder::new`] -- grounds the inputs against a state root.
//! - [`TxBuilder::begin_action`] / [`TxBuilder::end_action`] -- open and
//!   close an action scope. Direct events
//!   ([`TxBuilder::insert`] / [`TxBuilder::mutate`] / [`TxBuilder::delete`])
//!   emitted between them must each have guard evidence attached via
//!   [`TxBuilder::set_guard`] before the scope closes. Scopes nest:
//!   calling `begin_action` again before closing the first opens a
//!   sub-action whose events appear nested under the parent.
//! - [`TxBuilder::finalize`] -- walks the event tree and emits the
//!   `TxFinalized` proof.
//!
//! # Module layout
//!
//! - `object` -- object states and the values derived from them (the
//!   field accessors, the nullifier derivation, and the small dict
//!   transforms). No dependency on the builder.
//! - `state_header` -- the committed state view a transaction grounds
//!   against, and the witness carrying its membership proofs.
//! - `replay` -- the finalize-time walk over the recorded event tree.
//! - [`predicates`] -- the podlang sources and their compiled modules.
//! - [`test_support`] -- the scenario fixtures, a plain public module
//!   so a crate layered above this one sets a scenario up the same way.
//!
//! This module holds the rest: the recorded event tree, [`TxBuilder`],
//! and its `finalize`.

pub mod predicates;

mod object;
mod replay;
mod state_header;
pub mod test_support;

pub(crate) use object::OBJECT_NULLIFIER_VERSION;
pub use object::{
    STABLE_IDENTIFIER_FIELD, compute_nullifier, new_obj, object_key_hash,
    object_nullifier_from_key_hash, object_nullifier_hash, object_type, rekey,
    with_stable_identifier,
};
pub use state_header::{
    GroundingWitness, RECORD_STATE_HEADER_FIELDS, RECORD_STATE_HEADER_PODLANG,
    STATE_HEADER_BLOCK_HASH_SLOT, STATE_HEADER_BLOCK_NUMBER_SLOT,
    STATE_HEADER_BLOCK_TIMESTAMP_SLOT, STATE_HEADER_CREATED_SLOT, STATE_HEADER_NULLIFIERS_SLOT,
    STATE_HEADER_PRIOR_STATE_HISTORY_SLOT, StateHeader,
};

use std::sync::Arc;

use pod2::{
    frontend::Operation,
    middleware::{
        EMPTY_VALUE, Hash, NativeOperation, OperationAux, OperationType, Statement, StrKey, Value,
        containers::{Dictionary, Set},
        hash_values,
    },
};
use pod2utils::{dict, macros::BuildContext, map, op, set, st_custom};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

// ============================================================================
// Transaction output
// ============================================================================

/// Output of a finalized transaction. The live set is known to the prover
/// but private in the proof.
#[derive(Clone, Debug)]
pub struct Tx {
    pub live: Set,
    pub nullifiers: Set,
    /// The after_tx dictionary. Its commitment is tx_final (the value the
    /// relayer publishes). Contains live, nullifiers, chain_start, chain_end.
    pub ctx: Dictionary,
    pub state_header: Arc<StateHeader>,
}

impl Tx {
    /// The transaction's committed dictionary. Its commitment is tx_final,
    /// the value the relayer publishes for this transaction.
    pub fn dict(&self) -> Dictionary {
        self.ctx.clone()
    }

    /// Commitments of the objects this tx leaves live.
    pub fn live_commitments(&self) -> anyhow::Result<Vec<Hash>> {
        self.live
            .iter()
            .map(|entry| Ok(Hash(entry?.raw().0)))
            .collect()
    }

    /// The nullifiers this tx emits.
    pub fn nullifier_hashes(&self) -> anyhow::Result<Vec<Hash>> {
        self.nullifiers
            .iter()
            .map(|entry| Ok(Hash(entry?.raw().0)))
            .collect()
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TxSerde {
    live: Set,
    nullifiers: Set,
    ctx: Dictionary,
    state_header: StateHeader,
}

impl Serialize for Tx {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        TxSerde {
            live: self.live.clone(),
            nullifiers: self.nullifiers.clone(),
            ctx: self.ctx.clone(),
            state_header: (*self.state_header).clone(),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Tx {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let payload = TxSerde::deserialize(deserializer)?;
        Ok(Self {
            live: payload.live,
            nullifiers: payload.nullifiers,
            ctx: payload.ctx,
            state_header: Arc::new(payload.state_header),
        })
    }
}

// ============================================================================
// Event tree (for replay construction in finalize)
// ============================================================================

pub(crate) enum ChainEvent {
    Insert {
        new: Dictionary,
        /// Pre-identity dict from which `new` was derived via
        /// `with_stable_identifier`. Threaded into replay so TxInsert's
        /// `initial` public arg (the dict the action constructed) can be
        /// bound at replay time.
        initial: Dictionary,
        chain_after: Hash,
        /// The TxInsert statement emitted at record time. Replay
        /// references this directly instead of re-proving the chain
        /// step's hash equations.
        tx_stmt: Statement,
        guard_evidence: Option<Statement>,
    },
    Mutate {
        new: Dictionary,
        old: Dictionary,
        chain_after: Hash,
        /// The TxMutate statement emitted at record time.
        tx_stmt: Statement,
        guard_evidence: Option<Statement>,
    },
    Delete {
        old: Dictionary,
        chain_after: Hash,
        /// The TxDelete statement emitted at record time.
        tx_stmt: Statement,
        guard_evidence: Option<Statement>,
    },
    Action {
        chain_after: Hash,
        contents: Vec<ChainEvent>,
    },
}

struct ActionScope {
    events: Vec<ChainEvent>,
    scope_id: u64,
}

/// Opaque, Copy handle to a direct event emitted inside an action scope.
/// Pass to [`TxBuilder::set_guard`] to attach guard evidence. A handle
/// is only valid for the scope it was emitted in; using it after that
/// scope has closed (or in a different scope) panics with a
/// scope-mismatch message.
#[derive(Copy, Clone, Debug)]
pub struct EventHandle {
    scope_id: u64,
    index: usize,
}

// ============================================================================
// Replay tx-dict helpers
// ============================================================================

/// Build a replay tx dict with all 4 keys (chain is separate).
pub(crate) fn build_tx(
    live: &Set,
    nullifiers: &Set,
    chain_start: Hash,
    chain_end: Hash,
) -> Dictionary {
    dict!({
        "live" => live.clone(),
        "nullifiers" => nullifiers.clone(),
        "chain_start" => chain_start,
        "chain_end" => chain_end
    })
}

/// Return a clone of `tx` with one field replaced.
pub(crate) fn tx_with(tx: &Dictionary, key: &str, value: Value) -> Dictionary {
    let mut result = tx.clone();
    result.update(&StrKey::from(key), &value).unwrap();
    result
}

// ============================================================================
// TxBuilder
// ============================================================================

/// Predicate call counts from building a transaction.
pub type TxStats = std::collections::BTreeMap<String, usize>;

pub(crate) fn record(stats: &mut TxStats, name: &str) {
    *stats.entry(name.to_string()).or_default() += 1;
}

pub fn print_stats(stats: &TxStats) {
    let total: usize = stats.values().sum();
    println!("Predicate calls ({total} total):");
    for (name, count) in stats {
        println!("  {count:3}x {name}");
    }
}

pub struct TxBuilder {
    pub chain: Hash,
    pub chain_start: Hash,
    live: Set,
    nullifiers: Set,
    state_header: Arc<StateHeader>,
    st_inputs_grounded: Statement,
    inputs_set: Set,
    events: Vec<ChainEvent>,
    action_stack: Vec<ActionScope>,
    next_scope_id: u64,
    stats: TxStats,
}

// ============================================================================
// Display
// ============================================================================

/// Fields to skip in compact display (noise for debugging).
const DISPLAY_SKIP_FIELDS: &[&str] = &["type", "key", STABLE_IDENTIFIER_FIELD];

/// Format a Dictionary as a compact summary: commitment + interesting fields.
fn obj_summary(obj: &Dictionary) -> String {
    let prefix = format!("{}", obj.commitment());
    let mut fields = Vec::new();
    for entry in obj.iter() {
        let Ok((k, v)) = entry else { continue };
        if DISPLAY_SKIP_FIELDS.contains(&k.as_str()) {
            continue;
        }
        fields.push(format!("{k}: {v}"));
    }
    if fields.is_empty() {
        prefix
    } else {
        fields.sort();
        format!("{prefix} {{{}}}", fields.join(", "))
    }
}

/// Show which fields changed between old and new.
fn mutation_diff(old: &Dictionary, new: &Dictionary) -> String {
    let prefix = format!("{}", new.commitment());
    let mut diffs = Vec::new();
    for entry in new.iter() {
        let Ok((k, new_val)) = entry else { continue };
        if k == "type" {
            continue;
        }
        let old_val = old.get(&StrKey::from(&k)).ok().flatten();
        match old_val {
            Some(ov) if ov.raw() != new_val.raw() => {
                diffs.push(format!("{k}: {ov} -> {new_val}"));
            }
            None => {
                diffs.push(format!("+{k}: {new_val}"));
            }
            _ => {}
        }
    }
    if diffs.is_empty() {
        format!("{prefix} (no visible changes)")
    } else {
        diffs.sort();
        format!("{prefix} {{{}}}", diffs.join(", "))
    }
}

fn fmt_events(
    f: &mut std::fmt::Formatter<'_>,
    events: &[ChainEvent],
    indent: usize,
) -> std::fmt::Result {
    let pad = "  ".repeat(indent);
    for event in events {
        match event {
            ChainEvent::Insert { new, .. } => {
                writeln!(f, "{pad}insert {}", obj_summary(new))?;
            }
            ChainEvent::Mutate { old, new, .. } => {
                writeln!(f, "{pad}mutate {}", mutation_diff(old, new))?;
            }
            ChainEvent::Delete { old, .. } => {
                writeln!(f, "{pad}delete {}", obj_summary(old))?;
            }
            ChainEvent::Action { contents, .. } => {
                writeln!(f, "{pad}action")?;
                fmt_events(f, contents, indent + 1)?;
            }
        }
    }
    Ok(())
}

impl std::fmt::Display for TxBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Tx {} -> {}", self.chain_start, self.chain)?;
        fmt_events(f, &self.events, 1)?;

        // Live set
        let live_items: Vec<_> = self.live.iter().filter_map(|r| r.ok()).collect();
        if live_items.is_empty() {
            writeln!(f, "  live: (empty)")?;
        } else {
            writeln!(f, "  live: {} object(s)", live_items.len())?;
        }

        // Nullifiers
        let null_count = self.nullifiers.iter().filter(|r| r.is_ok()).count();
        if null_count > 0 {
            writeln!(f, "  nullifiers: {null_count}")?;
        }

        // Open scopes
        if !self.action_stack.is_empty() {
            writeln!(f, "  ({} open action scope(s))", self.action_stack.len())?;
        }

        Ok(())
    }
}

impl TxBuilder {
    /// Create a new transaction builder from grounded inputs.
    /// Seeds `chain_start = H(inputs, {})`.
    pub fn new(
        ctx: &mut BuildContext,
        inputs: &[Dictionary],
        grounding: Arc<GroundingWitness>,
    ) -> Self {
        let (st_inputs_grounded, inputs_set, stats) =
            Self::build_inputs_grounded(ctx, inputs, &grounding);
        let chain_start = hash_values(&[
            Value::from(inputs_set.commitment()),
            Value::from(EMPTY_VALUE),
        ]);
        let state_header = Arc::new(grounding.state_header.clone());
        Self {
            chain: chain_start,
            chain_start,
            live: inputs_set.clone(),
            nullifiers: set!(),
            state_header,
            st_inputs_grounded,
            inputs_set,
            events: vec![],
            action_stack: vec![],
            next_scope_id: 0,
            stats,
        }
    }

    pub fn chain_position(&self) -> Hash {
        self.chain
    }

    pub fn state_header(&self) -> &StateHeader {
        &self.state_header
    }

    /// Open a new action scope. Subsequent direct events
    /// (`insert`/`mutate`/`delete`) are recorded in this scope until
    /// `end_action` is called with the returned id. Scopes nest:
    /// calling `begin_action` again before closing the first opens a
    /// sub-action whose events appear nested under the parent.
    pub fn begin_action(&mut self) -> u64 {
        let scope_id = self.next_scope_id;
        self.next_scope_id += 1;
        self.action_stack.push(ActionScope {
            events: vec![],
            scope_id,
        });
        scope_id
    }

    /// Close the action scope identified by `scope_id`. Verifies that
    /// every direct event in the scope has guard evidence attached
    /// (panics on the first missing one), that the supplied id matches
    /// the top-of-stack scope, and that the scope is non-empty (the
    /// replay predicates only cover K>=1 bodies).
    pub fn end_action(&mut self, scope_id: u64) {
        self.verify_scope_guards(scope_id);
        let scope = self.action_stack.pop().expect("no action scope to close");
        assert_eq!(
            scope.scope_id, scope_id,
            "end_action scope id mismatch (expected {scope_id}, got {})",
            scope.scope_id
        );
        assert!(
            !scope.events.is_empty(),
            "end_action: action scope must contain at least one event"
        );
        self.push_event(ChainEvent::Action {
            chain_after: self.chain,
            contents: scope.events,
        });
    }

    /// Attach guard evidence to a previously emitted event. The handle
    /// must belong to the current (top-of-stack) scope; cross-scope
    /// handles panic.
    pub fn set_guard(&mut self, handle: EventHandle, guard: Statement) {
        let scope = self.action_stack.last_mut().expect("no open scope");
        assert_eq!(
            handle.scope_id, scope.scope_id,
            "EventHandle from a different scope (handle={}, current={})",
            handle.scope_id, scope.scope_id
        );
        let event = scope
            .events
            .get_mut(handle.index)
            .expect("event index out of range");
        match event {
            ChainEvent::Insert { guard_evidence, .. }
            | ChainEvent::Mutate { guard_evidence, .. }
            | ChainEvent::Delete { guard_evidence, .. } => {
                assert!(guard_evidence.is_none(), "guard evidence already set");
                *guard_evidence = Some(guard);
            }
            ChainEvent::Action { .. } => panic!("cannot set guard evidence on an action"),
        }
    }

    /// Check that every direct event in the named scope has guard
    /// evidence attached. Called by `end_action`; panics on the first
    /// unattached event found.
    fn verify_scope_guards(&self, scope_id: u64) {
        let scope = self.action_stack.last().expect("action scope missing");
        assert_eq!(scope.scope_id, scope_id);
        for (i, event) in scope.events.iter().enumerate() {
            match event {
                ChainEvent::Insert { guard_evidence, .. }
                | ChainEvent::Mutate { guard_evidence, .. }
                | ChainEvent::Delete { guard_evidence, .. } => {
                    assert!(
                        guard_evidence.is_some(),
                        "action scope {scope_id}: direct event {i} has no guard evidence"
                    );
                }
                ChainEvent::Action { .. } => {}
            }
        }
    }

    fn handle_for_last_event(&self) -> EventHandle {
        let scope = self.action_stack.last().expect("scope missing");
        let index = scope.events.len() - 1;
        EventHandle {
            scope_id: scope.scope_id,
            index,
        }
    }

    /// Record an insertion. Emits TxInsert, updates live set. Must be
    /// called inside an open action scope.
    ///
    /// `initial` is the pre-identity object state; the builder stamps
    /// `stable_identifer = commitment(initial)` and the returned
    /// `Dictionary` is the post-identity `new` that the tx records.
    /// Subsequent mutate/delete must reference the returned dict, not
    /// `initial`.
    pub fn insert(
        &mut self,
        ctx: &mut BuildContext,
        initial: &Dictionary,
    ) -> (Dictionary, Statement, EventHandle) {
        assert!(
            !self.action_stack.is_empty(),
            "insert must be called inside an action scope",
        );
        let new = with_stable_identifier(initial);

        let prev = self.chain;
        let event_hash = hash_values(&[Value::from(EMPTY_VALUE), Value::from(new.clone())]);
        self.chain = hash_values(&[Value::from(prev), Value::from(event_hash)]);
        self.live.insert(&Value::from(new.clone())).unwrap();

        let new_type = object_type(&new);
        let st_dc = ctx
            .builder
            .priv_op(op!(DictContains(new, "type", new_type.clone())))
            .unwrap();
        let stable_identifier = Value::from(initial.commitment());
        let st_di = ctx
            .builder
            .priv_op(op!(DictInsert(
                initial,
                STABLE_IDENTIFIER_FIELD,
                stable_identifier,
                new
            )))
            .unwrap();
        let st_h1 = ctx
            .builder
            .priv_op(op!(Hash(EMPTY_VALUE, new, event_hash)))
            .unwrap();
        let st_h2 = ctx
            .builder
            .priv_op(op!(Hash(prev, event_hash, self.chain)))
            .unwrap();
        let st = ctx
            .apply_custom_pred(
                false,
                "TxInsert",
                map!({"prev_chain" => prev, "chain" => self.chain, "initial" => initial.clone(), "new" => new.clone(), "type" => new_type}),
                vec![st_dc, st_di, st_h1, st_h2],
            )
            .unwrap();
        record(&mut self.stats, "TxInsert");

        self.push_event(ChainEvent::Insert {
            new: new.clone(),
            initial: initial.clone(),
            chain_after: self.chain,
            tx_stmt: st.clone(),
            guard_evidence: None,
        });
        let handle = self.handle_for_last_event();
        (new, st, handle)
    }

    /// Record a mutation. Emits TxMutate, updates live set and nullifiers.
    /// Must be called inside an open action scope. Returns the
    /// TxMutate statement and a handle for guard attachment.
    pub fn mutate(
        &mut self,
        ctx: &mut BuildContext,
        new: &Dictionary,
        old: &Dictionary,
    ) -> (Statement, EventHandle) {
        assert!(
            !self.action_stack.is_empty(),
            "mutate must be called inside an action scope",
        );
        let prev = self.chain;
        let event_hash = hash_values(&[Value::from(old.clone()), Value::from(new.clone())]);
        self.chain = hash_values(&[Value::from(prev), Value::from(event_hash)]);
        self.live.delete(&Value::from(old.commitment())).unwrap();
        self.live.insert(&Value::from(new.clone())).unwrap();
        self.nullifiers
            .insert(&Value::from(compute_nullifier(old)))
            .unwrap();

        let new_type = object_type(new);
        let old_type = object_type(old);
        assert_eq!(new_type, old_type, "mutate must preserve object type");
        let new_stable_identifier = new
            .get(&StrKey::from(STABLE_IDENTIFIER_FIELD))
            .expect("new dict lookup")
            .expect(
                "mutate target missing stable identifier field (must come from TxBuilder::insert)",
            );
        let old_stable_identifier = old
            .get(&StrKey::from(STABLE_IDENTIFIER_FIELD))
            .expect("old dict lookup")
            .expect(
                "mutate source missing stable identifier field (must come from TxBuilder::insert)",
            );
        assert_eq!(
            new_stable_identifier, old_stable_identifier,
            "mutate must preserve object stable identifier"
        );
        let st_dc_new = ctx
            .builder
            .priv_op(op!(DictContains(new, "type", new_type.clone())))
            .unwrap();
        let st_dc_old = ctx
            .builder
            .priv_op(op!(DictContains(old, "type", new_type.clone())))
            .unwrap();
        let st_eq_stable_identifier = ctx
            .builder
            .priv_op(op!(Equal(
                (old, STABLE_IDENTIFIER_FIELD),
                (new, STABLE_IDENTIFIER_FIELD)
            )))
            .unwrap();
        let st_h1 = ctx
            .builder
            .priv_op(op!(Hash(old, new, event_hash)))
            .unwrap();
        let st_h2 = ctx
            .builder
            .priv_op(op!(Hash(prev, event_hash, self.chain)))
            .unwrap();
        let st = ctx
            .apply_custom_pred(
                false,
                "TxMutate",
                map!({"prev_chain" => prev, "chain" => self.chain, "old" => old.clone(), "new" => new.clone(), "type" => new_type}),
                vec![st_dc_new, st_dc_old, st_eq_stable_identifier, st_h1, st_h2],
            )
            .unwrap();
        record(&mut self.stats, "TxMutate");

        self.push_event(ChainEvent::Mutate {
            new: new.clone(),
            old: old.clone(),
            chain_after: self.chain,
            tx_stmt: st.clone(),
            guard_evidence: None,
        });
        let handle = self.handle_for_last_event();
        (st, handle)
    }

    /// Record a deletion. Emits TxDelete, updates live set and nullifiers.
    /// Must be called inside an open action scope. Returns the
    /// TxDelete statement and a handle for guard attachment.
    pub fn delete(&mut self, ctx: &mut BuildContext, old: &Dictionary) -> (Statement, EventHandle) {
        assert!(
            !self.action_stack.is_empty(),
            "delete must be called inside an action scope",
        );
        let prev = self.chain;
        let event_hash = hash_values(&[Value::from(old.clone()), Value::from(EMPTY_VALUE)]);
        self.chain = hash_values(&[Value::from(prev), Value::from(event_hash)]);
        self.live.delete(&Value::from(old.commitment())).unwrap();
        self.nullifiers
            .insert(&Value::from(compute_nullifier(old)))
            .unwrap();

        let old_type = object_type(old);
        let st_dc = ctx
            .builder
            .priv_op(op!(DictContains(old, "type", old_type.clone())))
            .unwrap();
        let st_h1 = ctx
            .builder
            .priv_op(op!(Hash(old, EMPTY_VALUE, event_hash)))
            .unwrap();
        let st_h2 = ctx
            .builder
            .priv_op(op!(Hash(prev, event_hash, self.chain)))
            .unwrap();
        let st = ctx
            .apply_custom_pred(
                false,
                "TxDelete",
                map!({"prev_chain" => prev, "chain" => self.chain, "old" => old.clone(), "type" => old_type}),
                vec![st_dc, st_h1, st_h2],
            )
            .unwrap();
        record(&mut self.stats, "TxDelete");

        self.push_event(ChainEvent::Delete {
            old: old.clone(),
            chain_after: self.chain,
            tx_stmt: st.clone(),
            guard_evidence: None,
        });
        let handle = self.handle_for_last_event();
        (st, handle)
    }

    /// Build the replay chain and emit TxFinalized.
    pub fn finalize(self, ctx: &mut BuildContext) -> (Statement, Tx, TxStats) {
        assert!(self.action_stack.is_empty(), "unclosed action scopes");
        assert!(
            !self.events.is_empty(),
            "finalize: Tx must contain at least one top-level action"
        );

        let mut stats = self.stats;
        let zero: Hash = EMPTY_VALUE.into();

        let before_tx = build_tx(&self.inputs_set, &set!(), zero, zero);
        let after_tx = build_tx(&self.live, &self.nullifiers, zero, zero);

        // Replay the top-level action sequence. Every top-level event
        // is guaranteed to be a ChainEvent::Action (enforced by the
        // begin_action/end_action API), so we dispatch directly to
        // ReplayActions instead of going through ReplayContents.
        let empty_nullifiers = set!();
        let frame = replay::ReplayFrame {
            live: &self.inputs_set,
            nullifiers: &empty_nullifiers,
            chain_start: zero,
            chain_end: zero,
        };
        let (st_replay, _, _, _) = replay::Replayer::new(ctx, &mut stats).build_replay_actions(
            &self.events,
            self.chain_start,
            frame,
        );

        // Tie grounding to the public state root: rebind the InputsGrounded
        // statement's `inputs` to `before_tx.live` (the in-tx working set) and
        // its created set to `state_header.created`. The latter is the single
        // state-root array access anchoring the whole grounding tree. Two calls
        // rather than one because the entries are different anchored-key types:
        // a dict key and an array index.
        let st_inputs_rebound = ctx
            .builder
            .priv_op(Operation::replace_value_with_entry(
                vec![Some((&before_tx, "live")), None],
                self.st_inputs_grounded.clone(),
            ))
            .unwrap();
        let state_header_arr = self.state_header.array();
        let st_inputs_rebound = ctx
            .builder
            .priv_op(Operation::replace_value_with_entry(
                vec![
                    None,
                    Some((&state_header_arr, STATE_HEADER_CREATED_SLOT as i64)),
                ],
                st_inputs_rebound,
            ))
            .unwrap();
        let st_hash = ctx
            .builder
            .priv_op(op!(Hash(self.inputs_set, EMPTY_VALUE, self.chain_start)))
            .unwrap();
        let st_hash_rebound = ctx
            .builder
            .priv_op(Operation::replace_value_with_entry(
                vec![Some((&before_tx, "live")), None, None],
                st_hash,
            ))
            .unwrap();
        // Pin the full schema of `before_tx` (nullifiers={}, chain_start={},
        // chain_end={}, live=inputs_set) in a single DictInsert clause. This
        // closes the malleability where the prover could otherwise witness
        // arbitrary chain_start/chain_end values that pass through ReplayActions
        // verbatim into tx_final.
        let scope_dict = dict!({
            "nullifiers" => set!(),
            "chain_start" => zero,
            "chain_end" => zero
        });
        let st_dict_insert_lit = ctx
            .builder
            .priv_op(op!(DictInsert(
                scope_dict,
                "live",
                self.inputs_set,
                before_tx
            )))
            .unwrap();
        let st_dict_insert = ctx
            .builder
            .priv_op(Operation::replace_value_with_entry(
                vec![None, None, Some((&before_tx, "live")), None],
                st_dict_insert_lit,
            ))
            .unwrap();
        // Surface the final nullifier and live sets as public args.
        let st_dc_null_after = ctx
            .builder
            .priv_op(op!(DictContains(after_tx, "nullifiers", self.nullifiers)))
            .unwrap();
        let st_dc_live_after = ctx
            .builder
            .priv_op(op!(DictContains(after_tx, "live", self.live)))
            .unwrap();
        let st_bindings = ctx
            .apply_custom_pred_simple(
                false,
                "TxFinalBindings",
                vec![st_dc_null_after, st_dc_live_after],
            )
            .unwrap();
        record(&mut stats, "TxFinalBindings");
        let st = ctx
            .apply_custom_pred_simple(
                false,
                "TxFinalized",
                vec![
                    st_inputs_rebound,
                    st_hash_rebound,
                    st_dict_insert,
                    st_bindings,
                    st_replay,
                ],
            )
            .unwrap();
        record(&mut stats, "TxFinalized");

        let tx = Tx {
            live: self.live,
            nullifiers: self.nullifiers,
            ctx: after_tx,
            state_header: self.state_header,
        };
        (st, tx, stats)
    }

    // ========================================================================
    // Private
    // ========================================================================

    fn push_event(&mut self, event: ChainEvent) {
        if let Some(scope) = self.action_stack.last_mut() {
            scope.events.push(event);
        } else {
            self.events.push(event);
        }
    }

    fn build_inputs_grounded(
        ctx: &mut BuildContext,
        inputs: &[Dictionary],
        grounding: &GroundingWitness,
    ) -> (Statement, Set, TxStats) {
        let mut stats = TxStats::new();
        // Ground against the created-set commitment as a plain value; TxFinalized
        // is what ties it back to `state_header.created`.
        let created_root = grounding.state_header.created_root;
        let created_value = Value::from(created_root);

        if inputs.is_empty() {
            // Base case: empty inputs. `created` is unconstrained here.
            let st = st_custom!(
                ctx,
                InputsGrounded(created = created_value) = (
                    Equal(set!(), set!()),
                    Statement::None,
                    Statement::None,
                    Statement::None
                )
            )
            .unwrap();
            record(&mut stats, "InputsGrounded");
            return (st, set!(), stats);
        }

        let extend_set = |set: &Set, obj: &Dictionary| -> Set {
            let mut new_set = set.clone();
            new_set.insert(&Value::from(obj.clone())).unwrap();
            new_set
        };

        let prove_input = |ctx: &mut BuildContext, obj: &Dictionary| {
            prove_obj_in_created(ctx, created_root, grounding, obj)
        };

        // Bottom of the recursion: Single for odd N, Pair (both inputs inline)
        // for even N. Then peel two inputs per InputsGroundedRecursive level.
        let (mut st, mut prev_set, mut consumed) = if inputs.len() % 2 == 1 {
            let obj = &inputs[0];
            let inputs_set = extend_set(&set!(), obj);
            let st_live = prove_input(ctx, obj);
            let st_single = st_custom!(
                ctx,
                InputsGroundedSingle() = (st_live, SetInsert(set!(), obj, inputs_set))
            )
            .unwrap();
            record(&mut stats, "InputsGroundedSingle");
            let st = st_custom!(
                ctx,
                InputsGrounded(created = created_value) =
                    (Statement::None, st_single, Statement::None, Statement::None)
            )
            .unwrap();
            record(&mut stats, "InputsGrounded");
            (st, inputs_set, 1usize)
        } else {
            let first = &inputs[0];
            let second = &inputs[1];
            let set_first = extend_set(&set!(), first);
            let inputs_pair = extend_set(&set_first, second);
            let st_first = prove_input(ctx, first);
            let st_second = prove_input(ctx, second);
            let st_pair = st_custom!(
                ctx,
                InputsGroundedPair() = (
                    st_first,
                    SetInsert(set!(), first, set_first),
                    st_second,
                    SetInsert(set_first, second, inputs_pair)
                )
            )
            .unwrap();
            record(&mut stats, "InputsGroundedPair");
            let st = st_custom!(
                ctx,
                InputsGrounded(created = created_value) =
                    (Statement::None, Statement::None, st_pair, Statement::None)
            )
            .unwrap();
            record(&mut stats, "InputsGrounded");
            (st, inputs_pair, 2usize)
        };

        // Peel two inputs per recursion level.
        while consumed < inputs.len() {
            let first = &inputs[consumed];
            let second = &inputs[consumed + 1];
            let mid = extend_set(&prev_set, first);
            let next_set = extend_set(&mid, second);
            let st_first = prove_input(ctx, first);
            let st_second = prove_input(ctx, second);
            let st_rec = st_custom!(
                ctx,
                InputsGroundedRecursive() = (
                    st_first,
                    SetInsert(prev_set, first, mid),
                    st_second,
                    SetInsert(mid, second, next_set),
                    st
                )
            )
            .unwrap();
            record(&mut stats, "InputsGroundedRecursive");
            prev_set = next_set;
            consumed += 2;
            st = st_custom!(
                ctx,
                InputsGrounded(created = created_value) =
                    (Statement::None, Statement::None, Statement::None, st_rec)
            )
            .unwrap();
            record(&mut stats, "InputsGrounded");
        }
        (st, prev_set, stats)
    }
}

/// Prove `ArrayContains(created, index, obj)` for one input object against the
/// global created-set commitment `created_root`, passed as a plain literal.
///
/// The created set stores object commitments at sequential indices, but
/// `Value::from(obj)` hashes to that same commitment, so the per-object proof
/// lines up against the full object dict here. The index comes from the
/// grounding witness.
fn prove_obj_in_created(
    ctx: &mut BuildContext,
    created_root: Hash,
    grounding: &GroundingWitness,
    obj: &Dictionary,
) -> Statement {
    let (index, proof) = grounding
        .created_proofs
        .get(&obj.commitment())
        .cloned()
        .expect("missing created-set proof in grounding witness");
    ctx.builder
        .priv_op(Operation(
            OperationType::Native(NativeOperation::ArrayContainsFromEntries),
            vec![
                Value::from(created_root).into(),
                Value::from(index).into(),
                Value::from(obj.clone()).into(),
            ],
            OperationAux::MerkleProof(proof),
        ))
        .unwrap()
}

#[cfg(test)]
mod tests;
