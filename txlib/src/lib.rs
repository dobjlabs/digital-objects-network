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
//! - [`TxBuilder::new`] — grounds the inputs against a state root.
//! - [`TxBuilder::begin_action`] / [`TxBuilder::end_action`] — open and
//!   close an action scope. Direct events
//!   ([`TxBuilder::insert`] / [`TxBuilder::mutate`] / [`TxBuilder::delete`])
//!   emitted between them must each have guard evidence attached via
//!   [`TxBuilder::set_guard`] before the scope closes. Scopes nest:
//!   calling `begin_action` again before closing the first opens a
//!   sub-action whose events appear nested under the parent.
//! - [`TxBuilder::finalize`] — walks the event tree and emits the
//!   `TxFinalized` proof.
//!
//! The [`replay`] submodule contains the predicate-tree construction
//! invoked by `finalize`.

pub mod predicates;
mod replay;

use std::{collections::HashMap, sync::Arc};

use pod2::{
    backends::plonky2::primitives::merkletree::MerkleProof,
    frontend::{Operation, OperationArg},
    middleware::{
        EMPTY_VALUE, Hash, NativeOperation, OperationAux, OperationType, Statement, StrKey, Value,
        containers::{Dictionary, Set},
        hash_values,
    },
};
use pod2utils::{dict, macros::BuildContext, map, op, rand_raw_value, set, st_custom};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

// ============================================================================
// Data structures
// ============================================================================

/// Compact committed view of canonical app state used for grounding transactions.
///
/// Holds only the Merkle roots needed to recompute the canonical global state
/// root hash and to verify synchronizer-supplied membership proofs. Full
/// containers are not carried -- callers prove source-tx inclusion with
/// per-input Merkle proofs packaged in a [`GroundingWitness`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StateRoot {
    pub block_number: i64,
    pub transactions_root: Hash,
    pub nullifiers_root: Hash,
    pub gsrs_root: Hash,
}

impl StateRoot {
    pub fn new(
        block_number: i64,
        transactions_root: Hash,
        nullifiers_root: Hash,
        gsrs_root: Hash,
    ) -> Self {
        Self {
            block_number,
            transactions_root,
            nullifiers_root,
            gsrs_root,
        }
    }

    /// 3-layer hash:
    ///   H(H(txns_root, nullifiers_root), H(block_number, gsrs_root))
    pub fn hash(&self) -> Hash {
        let txn_nullifiers_hash = hash_values(&[
            Value::from(self.transactions_root),
            Value::from(self.nullifiers_root),
        ]);
        let block_number_gsrs_hash =
            hash_values(&[Value::from(self.block_number), Value::from(self.gsrs_root)]);
        hash_values(&[
            Value::from(txn_nullifiers_hash),
            Value::from(block_number_gsrs_hash),
        ])
    }
}

/// Proof-bearing grounding data required to build a new transaction.
///
/// Callers use `state_root` as the committed global context and
/// `source_tx_proofs` to prove that each consumed source transaction is
/// present in `state_root.transactions_root`.
#[derive(Clone, Debug)]
pub struct GroundingWitness {
    pub state_root: StateRoot,
    /// Merkle proofs for source transaction inclusion keyed by source tx
    /// commitment (`Tx::dict().commitment()`).
    pub source_tx_proofs: HashMap<Hash, MerkleProof>,
}

impl GroundingWitness {
    pub fn new(state_root: StateRoot, source_tx_proofs: HashMap<Hash, MerkleProof>) -> Self {
        Self {
            state_root,
            source_tx_proofs,
        }
    }
}

/// Output of a finalized transaction. The live set is known to the prover
/// but private in the proof.
#[derive(Clone, Debug)]
pub struct Tx {
    pub live: Set,
    pub nullifiers: Set,
    /// The after_tx dictionary. Its commitment is tx_final (stored in
    /// the state root's transactions set). Contains live, nullifiers,
    /// chain_start, chain_end.
    pub ctx: Dictionary,
    pub state_root: Arc<StateRoot>,
}

impl Tx {
    /// The transaction's committed dictionary. Its commitment is what
    /// gets stored in the state root's transactions set.
    pub fn dict(&self) -> Dictionary {
        self.ctx.clone()
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TxSerde {
    live: Set,
    nullifiers: Set,
    ctx: Dictionary,
    state_root: StateRoot,
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
            state_root: (*self.state_root).clone(),
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
            state_root: Arc::new(payload.state_root),
        })
    }
}

/// Per-object membership evidence: the source transaction's commitment,
/// the live-set commitment, and Merkle proofs that anchor the object
/// inside that source transaction's live set. Produced at mint time by
/// the source transaction's prover and packaged alongside each output
/// object; consumed at proof time by [`TxBuilder`] when the object is
/// spent.
///
/// State-root anchoring (proving the source tx is in `transactions_root`)
/// is supplied separately at consume time by the synchronizer via
/// [`GroundingWitness`]; it cannot be pre-computed at mint time because
/// the source tx is not yet anchored.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroundingEvidence {
    /// Source transaction commitment (== `source_tx.ctx.commitment()`).
    pub tx_final: Hash,
    /// Commitment of the source transaction's live set.
    pub live_root: Hash,
    /// Merkle proof for `DictContains(source_tx_ctx, "live", live_root)`.
    pub live_in_tx_proof: MerkleProof,
    /// Merkle proof for `SetContains(live, obj)`.
    pub obj_in_live_proof: MerkleProof,
}

impl GroundingEvidence {
    /// Construct from the source transaction's `ctx` dict and `live` set
    /// (both available to the prover at finalize time) plus the produced
    /// object whose membership we attest.
    pub fn new(ctx: &Dictionary, live: &Set, obj: &Dictionary) -> anyhow::Result<Self> {
        let tx_final = ctx.commitment();
        let live_root = live.commitment();
        let (_, live_in_tx_proof) = ctx.prove(&StrKey::from("live"))?;
        let obj_in_live_proof = live.prove(&Value::from(obj.clone()))?;
        Ok(Self {
            tx_final,
            live_root,
            live_in_tx_proof,
            obj_in_live_proof,
        })
    }
}

pub(crate) const OBJECT_NULLIFIER_VERSION: &str = "txlib-nullifier-v1";

pub fn object_key_hash(obj: &Dictionary) -> anyhow::Result<Hash> {
    let key = obj
        .get(&StrKey::from("key"))?
        .ok_or_else(|| anyhow::anyhow!("object missing required key field"))?;
    Ok(hash_values(&[Value::from(obj.commitment()), key]))
}

pub fn object_nullifier_from_key_hash(obj_key_hash: Hash) -> Hash {
    hash_values(&[
        Value::from(obj_key_hash),
        Value::from(OBJECT_NULLIFIER_VERSION),
    ])
}

pub fn object_nullifier_hash(obj: &Dictionary) -> anyhow::Result<Hash> {
    object_key_hash(obj).map(object_nullifier_from_key_hash)
}

/// Infallible variant used internally after keys have been validated.
/// H(H(obj, obj.key), "txlib-nullifier-v1")
pub fn compute_nullifier(obj: &Dictionary) -> Hash {
    object_nullifier_hash(obj).expect("object missing required key field")
}

pub fn rekey(obj: &mut Dictionary) {
    obj.update(&StrKey::from("key"), &Value::from(rand_raw_value()))
        .unwrap();
}

pub fn new_obj() -> Dictionary {
    let mut map = HashMap::new();
    map.insert(StrKey::from("key"), Value::from(rand_raw_value()));
    map.insert(StrKey::from("work"), Value::from(EMPTY_VALUE));
    Dictionary::new(map)
}

// ============================================================================
// Event tree (for replay construction in finalize)
// ============================================================================

pub(crate) enum ChainEvent {
    Insert {
        new: Dictionary,
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
    state_root: Arc<StateRoot>,
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
const DISPLAY_SKIP_FIELDS: &[&str] = &["type", "key"];

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
    /// Seeds `chain_start = H(inputs, 0)`.
    pub fn new(
        ctx: &mut BuildContext,
        inputs: &[(Dictionary, GroundingEvidence)],
        grounding: Arc<GroundingWitness>,
    ) -> Self {
        let (st_inputs_grounded, inputs_set, stats) =
            Self::build_inputs_grounded(ctx, inputs, &grounding);
        let chain_start = hash_values(&[
            Value::from(inputs_set.commitment()),
            Value::from(EMPTY_VALUE),
        ]);
        let state_root = Arc::new(grounding.state_root.clone());
        Self {
            chain: chain_start,
            chain_start,
            live: inputs_set.clone(),
            nullifiers: set!(),
            state_root,
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
    /// (panics on the first missing one) and that the supplied id
    /// matches the top-of-stack scope.
    pub fn end_action(&mut self, scope_id: u64) {
        self.verify_scope_guards(scope_id);
        let scope = self.action_stack.pop().expect("no action scope to close");
        assert_eq!(
            scope.scope_id, scope_id,
            "end_action scope id mismatch (expected {scope_id}, got {})",
            scope.scope_id
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

    /// Record an insertion. Emits TxInsert, updates live set.
    /// Must be called inside an open action scope. Returns the
    /// TxInsert statement (for composition into the action's
    /// predicate) and a handle used to attach guard evidence via
    /// `set_guard`.
    pub fn insert(&mut self, ctx: &mut BuildContext, new: &Dictionary) -> (Statement, EventHandle) {
        assert!(
            !self.action_stack.is_empty(),
            "insert must be called inside an action scope",
        );
        let prev = self.chain;
        let event_hash = hash_values(&[Value::from(EMPTY_VALUE), Value::from(new.clone())]);
        self.chain = hash_values(&[Value::from(prev), Value::from(event_hash)]);
        self.live.insert(&Value::from(new.clone())).unwrap();

        let st_h1 = ctx
            .builder
            .priv_op(op!(HashOf(event_hash, EMPTY_VALUE, new)))
            .unwrap();
        let st_h2 = ctx
            .builder
            .priv_op(op!(HashOf(self.chain, prev, event_hash)))
            .unwrap();
        let st = ctx
            .apply_custom_pred(
                false,
                "TxInsert",
                map!({"chain" => self.chain, "prev_chain" => prev, "new" => new.clone()}),
                vec![st_h1, st_h2],
            )
            .unwrap();
        record(&mut self.stats, "TxInsert");

        self.push_event(ChainEvent::Insert {
            new: new.clone(),
            chain_after: self.chain,
            tx_stmt: st.clone(),
            guard_evidence: None,
        });
        let handle = self.handle_for_last_event();
        (st, handle)
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

        let st_h1 = ctx
            .builder
            .priv_op(op!(HashOf(event_hash, old, new)))
            .unwrap();
        let st_h2 = ctx
            .builder
            .priv_op(op!(HashOf(self.chain, prev, event_hash)))
            .unwrap();
        let st = ctx
            .apply_custom_pred(
                false,
                "TxMutate",
                map!({"chain" => self.chain, "prev_chain" => prev, "new" => new.clone(), "old" => old.clone()}),
                vec![st_h1, st_h2],
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

        let st_h1 = ctx
            .builder
            .priv_op(op!(HashOf(event_hash, old, EMPTY_VALUE)))
            .unwrap();
        let st_h2 = ctx
            .builder
            .priv_op(op!(HashOf(self.chain, prev, event_hash)))
            .unwrap();
        let st = ctx
            .apply_custom_pred(
                false,
                "TxDelete",
                map!({"chain" => self.chain, "prev_chain" => prev, "old" => old.clone()}),
                vec![st_h1, st_h2],
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

        let mut stats = self.stats;
        let zero: Hash = EMPTY_VALUE.into();

        let before_tx = build_tx(&self.inputs_set, &set!(), zero, zero);
        let after_tx = build_tx(&self.live, &self.nullifiers, zero, zero);

        // Replay the top-level action sequence. Every top-level event
        // is guaranteed to be a ChainEvent::Action (enforced by the
        // begin_action/end_action API), so we dispatch directly to
        // ReplayActions instead of going through ReplayContents.
        let (st_replay, _, _, _) = replay::build_replay_actions(
            ctx,
            &mut stats,
            &self.events,
            self.chain_start,
            &self.inputs_set,
            &set!(),
            zero,
            zero,
        );

        // TxFinalized -- rebind inputs_grounded and chain_start hash
        // to reference before_tx.live instead of a literal inputs set.
        let st_inputs_rebound = ctx
            .builder
            .priv_op(Operation::replace_value_with_entry(
                vec![Some((&before_tx, "live")), None],
                self.st_inputs_grounded.clone(),
            ))
            .unwrap();
        let st_hash = ctx
            .builder
            .priv_op(op!(HashOf(self.chain_start, self.inputs_set, EMPTY_VALUE)))
            .unwrap();
        let st_hash_rebound = ctx
            .builder
            .priv_op(Operation::replace_value_with_entry(
                vec![None, Some((&before_tx, "live")), None],
                st_hash,
            ))
            .unwrap();
        // Pin the full schema of `before_tx` (nullifiers={}, chain_start=0,
        // chain_end=0, live=inputs_set) in a single DictInsert clause. This
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
                before_tx,
                scope_dict,
                "live",
                self.inputs_set
            )))
            .unwrap();
        let st_dict_insert = ctx
            .builder
            .priv_op(Operation::replace_value_with_entry(
                vec![None, None, None, Some((&before_tx, "live"))],
                st_dict_insert_lit,
            ))
            .unwrap();
        let st_dc_null_after = ctx
            .builder
            .priv_op(op!(DictContains(after_tx, "nullifiers", self.nullifiers)))
            .unwrap();
        let st = ctx
            .apply_custom_pred_simple(
                false,
                "TxFinalized",
                vec![
                    st_inputs_rebound,
                    st_hash_rebound,
                    st_dict_insert,
                    st_dc_null_after,
                    st_replay,
                ],
            )
            .unwrap();
        record(&mut stats, "TxFinalized");

        let tx = Tx {
            live: self.live,
            nullifiers: self.nullifiers,
            ctx: after_tx,
            state_root: self.state_root,
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
        inputs: &[(Dictionary, GroundingEvidence)],
        grounding: &GroundingWitness,
    ) -> (Statement, Set, TxStats) {
        let mut stats = TxStats::new();
        let state_root = &grounding.state_root;
        let state_root_hash = state_root.hash();
        let block_number = state_root.block_number;
        let transactions_root = state_root.transactions_root;
        let nullifiers_root = state_root.nullifiers_root;
        let gsrs_root = state_root.gsrs_root;

        if inputs.is_empty() {
            // Base case: empty inputs. state_root_hash is unconstrained here.
            let st = st_custom!(
                ctx,
                InputsGrounded(state_root_hash = state_root_hash) =
                    (Equal(set!(), set!()), Statement::None, Statement::None)
            )
            .unwrap();
            record(&mut stats, "InputsGrounded");
            return (st, set!(), stats);
        }

        // Intermediate hashes for the 3-layer state-root commitment.
        let txn_null_hash =
            hash_values(&[Value::from(transactions_root), Value::from(nullifiers_root)]);
        let bn_gsrs_hash = hash_values(&[Value::from(block_number), Value::from(gsrs_root)]);

        // Build the three HashOf statements that unpack the state-root
        // hash once; reused as sub-clauses by each TxInStateRoot below.
        let st_h_txn_null = ctx
            .builder
            .priv_op(op!(HashOf(
                txn_null_hash,
                transactions_root,
                nullifiers_root
            )))
            .unwrap();
        let st_h_bn_gsrs = ctx
            .builder
            .priv_op(op!(HashOf(bn_gsrs_hash, block_number, gsrs_root)))
            .unwrap();
        let st_h_state_root = ctx
            .builder
            .priv_op(op!(HashOf(state_root_hash, txn_null_hash, bn_gsrs_hash)))
            .unwrap();

        if inputs.len() == 1 {
            // Single-input fast path: InputsGroundedSingle avoids recursion.
            let (obj, evidence) = &inputs[0];
            let mut inputs_set = set!();
            inputs_set.insert(&Value::from(obj.clone())).unwrap();
            let st_tx_membership =
                prove_source_tx_membership(ctx, grounding, transactions_root, evidence.tx_final);
            let st_tx_in_sr = st_custom!(
                ctx,
                TxInStateRoot() = (
                    st_h_txn_null.clone(),
                    st_h_bn_gsrs.clone(),
                    st_h_state_root.clone(),
                    st_tx_membership
                )
            )
            .unwrap();
            record(&mut stats, "TxInStateRoot");
            let st_set_contains_live = prove_obj_in_source_tx_live(ctx, evidence, obj);
            let st_single = st_custom!(
                ctx,
                InputsGroundedSingle() = (
                    st_tx_in_sr,
                    st_set_contains_live,
                    SetInsert(inputs_set, set!(), obj)
                )
            )
            .unwrap();
            record(&mut stats, "InputsGroundedSingle");
            let st = st_custom!(
                ctx,
                InputsGrounded(state_root_hash = state_root_hash) =
                    (Statement::None, st_single, Statement::None)
            )
            .unwrap();
            record(&mut stats, "InputsGrounded");
            return (st, inputs_set, stats);
        }

        let mut st = st_custom!(
            ctx,
            InputsGrounded(state_root_hash = state_root_hash) =
                (Equal(set!(), set!()), Statement::None, Statement::None)
        )
        .unwrap();
        record(&mut stats, "InputsGrounded");
        let mut prev_set = set!();
        for (obj, evidence) in inputs {
            let mut next_set = prev_set.clone();
            next_set.insert(&Value::from(obj.clone())).unwrap();
            let st_tx_membership =
                prove_source_tx_membership(ctx, grounding, transactions_root, evidence.tx_final);
            let st_tx_in_sr = st_custom!(
                ctx,
                TxInStateRoot() = (
                    st_h_txn_null.clone(),
                    st_h_bn_gsrs.clone(),
                    st_h_state_root.clone(),
                    st_tx_membership
                )
            )
            .unwrap();
            record(&mut stats, "TxInStateRoot");
            let st_set_contains_live = prove_obj_in_source_tx_live(ctx, evidence, obj);
            let st_rec = st_custom!(
                ctx,
                InputsGroundedRecursive() = (
                    st_tx_in_sr,
                    st_set_contains_live,
                    SetInsert(next_set, prev_set, obj),
                    st
                )
            )
            .unwrap();
            record(&mut stats, "InputsGroundedRecursive");
            prev_set = next_set;
            st = st_custom!(
                ctx,
                InputsGrounded() = (Statement::None, Statement::None, st_rec)
            )
            .unwrap();
            record(&mut stats, "InputsGrounded");
        }
        (st, prev_set, stats)
    }
}

/// Prove `SetContains(transactions_root, tx_final)` using a Merkle proof
/// supplied by the grounding witness. The source tx is identified by its
/// commitment alone (the literal `ctx` dict is not required).
fn prove_source_tx_membership(
    ctx: &mut BuildContext,
    grounding: &GroundingWitness,
    transactions_root: Hash,
    tx_final: Hash,
) -> Statement {
    let proof = grounding
        .source_tx_proofs
        .get(&tx_final)
        .cloned()
        .expect("missing source tx proof in grounding witness");
    ctx.builder
        .priv_op(Operation(
            OperationType::Native(NativeOperation::SetContainsFromEntries),
            vec![transactions_root.into(), Value::from(tx_final).into()],
            OperationAux::MerkleProof(proof),
        ))
        .unwrap()
}

/// Build `SetContains(source_tx.live, obj)` from a [`GroundingEvidence`].
///
/// Discharges two Merkle proofs:
///   * `DictContains(source_tx_ctx, "live", live_root)` via `live_in_tx_proof`.
///   * `SetContains(live_root, obj)` via `obj_in_live_proof`.
///
/// The DictContains statement is fed as the first arg of the SetContains
/// op as `OperationArg::Statement(...)`. Pod2 then promotes that arg to
/// `ValueRef::Key(AnchoredKey(tx_final, "live"))`, which is the form the
/// `source_tx.live` template arg requires.
fn prove_obj_in_source_tx_live(
    ctx: &mut BuildContext,
    evidence: &GroundingEvidence,
    obj: &Dictionary,
) -> Statement {
    let st_dict_contains = ctx
        .builder
        .priv_op(Operation(
            OperationType::Native(NativeOperation::DictContainsFromEntries),
            vec![
                Value::from(evidence.tx_final).into(),
                Value::from("live").into(),
                Value::from(evidence.live_root).into(),
            ],
            OperationAux::MerkleProof(evidence.live_in_tx_proof.clone()),
        ))
        .unwrap();
    ctx.builder
        .priv_op(Operation(
            OperationType::Native(NativeOperation::SetContainsFromEntries),
            vec![
                OperationArg::Statement(st_dict_contains),
                Value::from(obj.clone()).into(),
            ],
            OperationAux::MerkleProof(evidence.obj_in_live_proof.clone()),
        ))
        .unwrap()
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use hex::FromHex;
    use pod2::{
        backends::plonky2::mock::mainpod::MockProver,
        frontend::{MainPod, MultiPodBuilder},
        middleware::{Params, Predicate, VDSet, containers::Array},
    };
    use pod2utils::{macros::BuildContext, set};

    use super::*;

    /// Running test state that mirrors what the synchronizer tracks. Keeps
    /// full transactions/nullifier sets on the prover side so it can hand
    /// out real Merkle proofs; exposes the canonical commitments-only
    /// `StateRoot` to callers.
    struct TestState {
        block_number: i64,
        transactions: Set,
        nullifiers: Set,
        gsrs: Array,
    }

    impl TestState {
        fn empty(block_number: i64) -> Self {
            Self {
                block_number,
                transactions: set!(),
                nullifiers: set!(),
                gsrs: Array::new(Vec::new()),
            }
        }

        fn state_root(&self) -> StateRoot {
            StateRoot::new(
                self.block_number,
                self.transactions.commitment(),
                self.nullifiers.commitment(),
                self.gsrs.commitment(),
            )
        }

        fn apply_tx(&mut self, tx: &Tx) {
            self.transactions
                .insert(&Value::from(tx.ctx.clone()))
                .unwrap();
            for nullifier in tx.nullifiers.iter() {
                let nullifier = nullifier.expect("tx nullifier should decode");
                self.nullifiers.insert(&nullifier).unwrap();
            }
        }

        fn grounding_witness(&self, source_txs: &[Tx]) -> Arc<GroundingWitness> {
            let source_tx_proofs = source_txs
                .iter()
                .map(|tx| {
                    let commitment = tx.ctx.commitment();
                    let proof = self
                        .transactions
                        .prove(&Value::from(commitment))
                        .expect("source tx should be provable from test state");
                    (commitment, proof)
                })
                .collect();
            Arc::new(GroundingWitness::new(self.state_root(), source_tx_proofs))
        }
    }

    fn solve_and_verify(builder: MultiPodBuilder) -> MainPod {
        eprintln!("resource summary: {}", builder.resource_summary());
        let solution = builder.solve().unwrap();
        eprintln!("solution: {}", solution.solution_breakdown());
        let pod = solution.prove(&MockProver {}).unwrap().output_pod().clone();
        pod.pod.verify().unwrap();
        pod
    }

    fn make_object(guard_hash: Value, fields: &[(&str, Value)]) -> Dictionary {
        let mut d = dict!({
            "type" => guard_hash,
            "key" => rand_raw_value()
        });
        for (k, v) in fields {
            d.insert(&StrKey::from(*k), v).unwrap();
        }
        d
    }

    fn test_hash(byte: u8) -> Hash {
        Hash::from_hex(hex::encode([byte; 32])).expect("valid test hash")
    }

    #[test]
    fn object_nullifier_hash_matches_key_hash_path() {
        let obj = new_obj();
        let key_hash = object_key_hash(&obj).unwrap();
        let nullifier = object_nullifier_hash(&obj).unwrap();
        assert_eq!(nullifier, object_nullifier_from_key_hash(key_hash));
        assert_eq!(nullifier, compute_nullifier(&obj));
    }

    #[test]
    fn object_nullifier_hash_errors_without_key() {
        let mut obj = new_obj();
        obj.delete(&StrKey::from("key")).unwrap();
        let err = object_nullifier_hash(&obj).expect_err("missing key must fail");
        assert!(format!("{err}").contains("missing required key field"));
    }

    #[test]
    fn state_root_hash_matches_legacy_commitments() {
        let txns = [test_hash(1), test_hash(2)]
            .into_iter()
            .collect::<HashSet<_>>();
        let nullifiers = [test_hash(3)].into_iter().collect::<HashSet<_>>();
        let prior_gsrs = vec![test_hash(4), test_hash(5)];

        let txs = Set::new(txns.iter().map(|hash| Value::from(*hash)).collect());
        let nulls = Set::new(nullifiers.iter().map(|hash| Value::from(*hash)).collect());
        let gsrs = Array::new(prior_gsrs.iter().map(|hash| Value::from(*hash)).collect());
        let compact = StateRoot::new(7, txs.commitment(), nulls.commitment(), gsrs.commitment());
        let legacy_hash = hash_values(&[
            Value::from(hash_values(&[
                Value::from(txs.commitment()),
                Value::from(nulls.commitment()),
            ])),
            Value::from(hash_values(&[
                Value::from(7_i64),
                Value::from(gsrs.commitment()),
            ])),
        ]);
        assert_eq!(compact.hash(), legacy_hash);
    }

    #[test]
    fn state_root_serializes_and_deserializes_camelcase() {
        let original = StateRoot::new(9, test_hash(1), test_hash(2), test_hash(3));
        let encoded = serde_json::to_value(&original).unwrap();
        assert_eq!(encoded["blockNumber"], serde_json::json!(9));
        assert_eq!(
            encoded["transactionsRoot"],
            serde_json::json!(hex::encode([1_u8; 32]))
        );
        assert_eq!(
            encoded["nullifiersRoot"],
            serde_json::json!(hex::encode([2_u8; 32]))
        );
        assert_eq!(
            encoded["gsrsRoot"],
            serde_json::json!(hex::encode([3_u8; 32]))
        );

        let decoded: StateRoot = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn test_tx_builder_empty() {
        let modules = vec![Arc::new(crate::predicates::module())];
        let state = TestState::empty(0);
        let params = Params::default();
        let vd_set = VDSet::new(&[]);
        let builder = MultiPodBuilder::new(&params, &vd_set);
        let mut ctx = BuildContext { builder, modules };

        let tx = TxBuilder::new(&mut ctx, &[], state.grounding_witness(&[]));
        let (st, tx, stats) = tx.finalize(&mut ctx);
        print_stats(&stats);
        ctx.builder.reveal(&st).unwrap();

        solve_and_verify(ctx.builder);
        assert!(tx.live.iter().next().is_none());
        assert!(tx.nullifiers.iter().next().is_none());
    }

    /// Tx 1: Spawn a WoodPick (insert, no inputs).
    /// Tx 2: MineStone using the WoodPick (mutate pick + insert stone).
    #[test]
    fn test_mine_stone() {
        let txlib = Arc::new(crate::predicates::module());
        let craft = Arc::new(crate::predicates::crafting_test_module());
        let modules = vec![txlib.clone(), craft.clone()];

        let is_wood_pick = Value::from(
            Predicate::Custom(craft.predicate_ref_by_name("IsWoodPick").unwrap()).hash(),
        );
        let is_stone =
            Value::from(Predicate::Custom(craft.predicate_ref_by_name("IsStone").unwrap()).hash());

        let mut state = TestState::empty(0);
        let params = Params::default();
        let vd_set = VDSet::new(&[]);

        // ---- Tx 1: Spawn a WoodPick ----

        let builder = MultiPodBuilder::new(&params, &vd_set);
        let mut ctx = BuildContext {
            builder,
            modules: modules.clone(),
        };

        let pick = make_object(
            is_wood_pick.clone(),
            &[("durability", Value::from(100_i64))],
        );

        let mut tx1 = TxBuilder::new(&mut ctx, &[], state.grounding_witness(&[]));

        let scope = tx1.begin_action();
        let (st_insert, h) = tx1.insert(&mut ctx, &pick);
        let op_type = ctx
            .builder
            .priv_op(op!(DictContains(pick, "type", is_wood_pick.clone())))
            .unwrap();
        let op_dur = ctx
            .builder
            .priv_op(op!(DictContains(pick, "durability", 100_i64)))
            .unwrap();
        let st_spawn = ctx
            .apply_custom_pred_simple(false, "SpawnWoodPick", vec![op_type, op_dur, st_insert])
            .unwrap();
        let st_guard = ctx
            .apply_custom_pred_simple(
                false,
                "IsWoodPick",
                vec![st_spawn.clone(), Statement::None, Statement::None],
            )
            .unwrap();
        tx1.set_guard(h, st_guard);
        tx1.end_action(scope);

        eprintln!("{tx1}");
        let (st, tx0, stats) = tx1.finalize(&mut ctx);
        print_stats(&stats);
        ctx.builder.reveal(&st).unwrap();
        solve_and_verify(ctx.builder);

        state.apply_tx(&tx0);

        // ---- Tx 2: MineStone ----

        let builder = MultiPodBuilder::new(&params, &vd_set);
        let mut ctx = BuildContext { builder, modules };

        let mut pick_new = pick.clone();
        pick_new
            .update(&StrKey::from("durability"), &Value::from(99_i64))
            .unwrap();
        let stone = make_object(is_stone.clone(), &[]);

        let witness = state.grounding_witness(std::slice::from_ref(&tx0));
        let evidence = GroundingEvidence::new(&tx0.ctx, &tx0.live, &pick).unwrap();
        let inputs = vec![(pick.clone(), evidence)];
        let mut tx2 = TxBuilder::new(&mut ctx, &inputs, witness);

        let scope_outer = tx2.begin_action();

        // Sub-action: UseWoodPick (mutate pick)
        let st_use_wp = {
            let scope_sub = tx2.begin_action();
            let (st_mutate, h_sub) = tx2.mutate(&mut ctx, &pick_new, &pick);
            let pick_type = pick.get(&StrKey::from("type")).unwrap().unwrap();
            let op_type = ctx
                .builder
                .priv_op(op!(DictContains(pick, "type", pick_type)))
                .unwrap();
            let op_gt = ctx
                .builder
                .priv_op(op!(Gt((&pick, "durability"), 0_i64)))
                .unwrap();
            let op_sum = ctx
                .builder
                .priv_op(op!(SumOf((&pick, "durability"), 99_i64, 1_i64)))
                .unwrap();
            let op_du = ctx
                .builder
                .priv_op(op!(DictUpdate(pick_new, pick, "durability", 99_i64)))
                .unwrap();
            let st_action = ctx
                .apply_custom_pred_simple(
                    false,
                    "UseWoodPick",
                    vec![op_type, op_gt, op_sum, op_du, st_mutate],
                )
                .unwrap();
            let st_guard = ctx
                .apply_custom_pred_simple(
                    false,
                    "IsWoodPick",
                    vec![Statement::None, Statement::None, st_action.clone()],
                )
                .unwrap();
            tx2.set_guard(h_sub, st_guard);
            tx2.end_action(scope_sub);
            st_action
        };

        // Direct: insert stone
        let (st_stone_insert, h) = tx2.insert(&mut ctx, &stone);
        let op_type = ctx
            .builder
            .priv_op(op!(DictContains(stone, "type", is_stone.clone())))
            .unwrap();
        let st_mine = ctx
            .apply_custom_pred_simple(
                false,
                "MineStone",
                vec![st_use_wp, op_type, st_stone_insert],
            )
            .unwrap();
        let st_guard = ctx
            .apply_custom_pred_simple(false, "IsStone", vec![st_mine.clone()])
            .unwrap();
        tx2.set_guard(h, st_guard);
        tx2.end_action(scope_outer);

        eprintln!("{tx2}");
        let (st, tx_out, stats) = tx2.finalize(&mut ctx);
        print_stats(&stats);
        ctx.builder.reveal(&st).unwrap();
        solve_and_verify(ctx.builder);

        assert!(
            tx_out
                .nullifiers
                .contains(&Value::from(compute_nullifier(&pick)))
                .unwrap()
        );
    }

    /// Tx 1: FindLog (genesis insert).
    /// Tx 2: CraftWood (delete log, insert wood).
    /// Tx 3: CraftSticks (delete wood, insert two sticks).
    #[test]
    fn test_craft_sticks() {
        let txlib = Arc::new(crate::predicates::module());
        let craft = Arc::new(crate::predicates::crafting_test_module());
        let modules = vec![txlib.clone(), craft.clone()];

        let is_log =
            Value::from(Predicate::Custom(craft.predicate_ref_by_name("IsLog").unwrap()).hash());
        let is_wood =
            Value::from(Predicate::Custom(craft.predicate_ref_by_name("IsWood").unwrap()).hash());
        let is_stick =
            Value::from(Predicate::Custom(craft.predicate_ref_by_name("IsStick").unwrap()).hash());

        let mut state = TestState::empty(0);
        let params = Params::default();
        let vd_set = VDSet::new(&[]);

        // ---- Tx 1: FindLog ----

        let builder = MultiPodBuilder::new(&params, &vd_set);
        let mut ctx = BuildContext {
            builder,
            modules: modules.clone(),
        };

        let log = make_object(is_log.clone(), &[]);

        let mut tx1 = TxBuilder::new(&mut ctx, &[], state.grounding_witness(&[]));

        let scope = tx1.begin_action();
        let (st_insert, h) = tx1.insert(&mut ctx, &log);
        let op_type = ctx
            .builder
            .priv_op(op!(DictContains(log, "type", is_log.clone())))
            .unwrap();
        let st_find = ctx
            .apply_custom_pred_simple(false, "FindLog", vec![op_type, st_insert])
            .unwrap();
        let st_guard = ctx
            .apply_custom_pred_simple(false, "IsLog", vec![st_find.clone(), Statement::None])
            .unwrap();
        tx1.set_guard(h, st_guard);
        tx1.end_action(scope);

        eprintln!("{tx1}");
        let (st, tx1_out, stats) = tx1.finalize(&mut ctx);
        print_stats(&stats);
        ctx.builder.reveal(&st).unwrap();
        solve_and_verify(ctx.builder);

        state.apply_tx(&tx1_out);

        // ---- Tx 2: CraftWood ----

        let builder = MultiPodBuilder::new(&params, &vd_set);
        let mut ctx = BuildContext {
            builder,
            modules: modules.clone(),
        };

        let wood = make_object(is_wood.clone(), &[]);

        let witness = state.grounding_witness(std::slice::from_ref(&tx1_out));
        let evidence = GroundingEvidence::new(&tx1_out.ctx, &tx1_out.live, &log).unwrap();
        let inputs = vec![(log.clone(), evidence)];
        let mut tx2 = TxBuilder::new(&mut ctx, &inputs, witness);

        let scope_outer = tx2.begin_action();

        // Sub-action: DeleteLog
        let st_del_log = {
            let scope_sub = tx2.begin_action();
            let (st_del, h_sub) = tx2.delete(&mut ctx, &log);
            let log_type = log.get(&StrKey::from("type")).unwrap().unwrap();
            let op_type = ctx
                .builder
                .priv_op(op!(DictContains(log, "type", log_type)))
                .unwrap();
            let st_action = ctx
                .apply_custom_pred_simple(false, "DeleteLog", vec![op_type, st_del])
                .unwrap();
            let st_guard = ctx
                .apply_custom_pred_simple(false, "IsLog", vec![Statement::None, st_action.clone()])
                .unwrap();
            tx2.set_guard(h_sub, st_guard);
            tx2.end_action(scope_sub);
            st_action
        };

        // Direct: insert wood
        let (st_ins, h) = tx2.insert(&mut ctx, &wood);
        let op_type = ctx
            .builder
            .priv_op(op!(DictContains(wood, "type", is_wood.clone())))
            .unwrap();
        let st_craft_wood = ctx
            .apply_custom_pred_simple(false, "CraftWood", vec![st_del_log, op_type, st_ins])
            .unwrap();
        let st_guard = ctx
            .apply_custom_pred_simple(
                false,
                "IsWood",
                vec![st_craft_wood.clone(), Statement::None],
            )
            .unwrap();
        tx2.set_guard(h, st_guard);
        tx2.end_action(scope_outer);

        eprintln!("{tx2}");
        let (st, tx2_out, stats) = tx2.finalize(&mut ctx);
        print_stats(&stats);
        ctx.builder.reveal(&st).unwrap();
        solve_and_verify(ctx.builder);

        state.apply_tx(&tx2_out);

        // ---- Tx 3: CraftSticks ----

        let builder = MultiPodBuilder::new(&params, &vd_set);
        let mut ctx = BuildContext { builder, modules };

        let stick_a = make_object(is_stick.clone(), &[]);
        let stick_b = make_object(is_stick, &[]);

        let witness = state.grounding_witness(std::slice::from_ref(&tx2_out));
        let evidence = GroundingEvidence::new(&tx2_out.ctx, &tx2_out.live, &wood).unwrap();
        let inputs = vec![(wood.clone(), evidence)];
        let mut tx3 = TxBuilder::new(&mut ctx, &inputs, witness);

        let scope_outer = tx3.begin_action();

        // Sub-action: DeleteWood
        let st_del_wood = {
            let scope_sub = tx3.begin_action();
            let (st_del, h_sub) = tx3.delete(&mut ctx, &wood);
            let wood_type = wood.get(&StrKey::from("type")).unwrap().unwrap();
            let op_type = ctx
                .builder
                .priv_op(op!(DictContains(wood, "type", wood_type)))
                .unwrap();
            let st_action = ctx
                .apply_custom_pred_simple(false, "DeleteWood", vec![op_type, st_del])
                .unwrap();
            let st_guard = ctx
                .apply_custom_pred_simple(false, "IsWood", vec![Statement::None, st_action.clone()])
                .unwrap();
            tx3.set_guard(h_sub, st_guard);
            tx3.end_action(scope_sub);
            st_action
        };

        // Direct: insert stick_a
        let (st_ins_a, h_a) = tx3.insert(&mut ctx, &stick_a);
        let stick_type = stick_a.get(&StrKey::from("type")).unwrap().unwrap();
        let op_type_a = ctx
            .builder
            .priv_op(op!(DictContains(stick_a, "type", stick_type.clone())))
            .unwrap();

        // Direct: insert stick_b
        let (st_ins_b, h_b) = tx3.insert(&mut ctx, &stick_b);
        let op_type_b = ctx
            .builder
            .priv_op(op!(DictContains(stick_b, "type", stick_type)))
            .unwrap();

        let st_craft_sticks = ctx
            .apply_custom_pred_simple(
                false,
                "CraftSticks",
                vec![st_del_wood, op_type_a, st_ins_a, op_type_b, st_ins_b],
            )
            .unwrap();

        // stick_a: IsStick branch 2 = CraftSticks(obj, other, chain_start, chain_end)
        let st_is_stick_a = ctx
            .apply_custom_pred_simple(
                false,
                "IsStick",
                vec![Statement::None, st_craft_sticks.clone(), Statement::None],
            )
            .unwrap();
        tx3.set_guard(h_a, st_is_stick_a);

        // stick_b: IsStick branch 3 = CraftSticks(other, obj, chain_start, chain_end)
        let st_is_stick_b = ctx
            .apply_custom_pred_simple(
                false,
                "IsStick",
                vec![Statement::None, Statement::None, st_craft_sticks.clone()],
            )
            .unwrap();
        tx3.set_guard(h_b, st_is_stick_b);

        tx3.end_action(scope_outer);

        eprintln!("{tx3}");
        let (st, tx3_out, stats) = tx3.finalize(&mut ctx);
        print_stats(&stats);
        ctx.builder.reveal(&st).unwrap();
        solve_and_verify(ctx.builder);

        // Both sticks should be live
        assert!(tx3_out.live.contains(&Value::from(stick_a)).unwrap());
        assert!(tx3_out.live.contains(&Value::from(stick_b)).unwrap());
        // Wood should be nullified
        assert!(
            tx3_out
                .nullifiers
                .contains(&Value::from(compute_nullifier(&wood)))
                .unwrap()
        );
    }
}
