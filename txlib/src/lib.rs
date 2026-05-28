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
        containers::{Array, Dictionary, Set},
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

    /// Padded-array view used as the canonical state root record. Slot
    /// layout matches the `record StateRoot` declaration in txlib.podlang.
    /// Predicates access fields via anchored-key syntax (e.g.
    /// `state_root.transactions`).
    pub fn array(&self) -> Array {
        Array::new(vec![
            Value::from(0_i64),
            Value::from(self.block_number),
            Value::from(self.transactions_root),
            Value::from(self.nullifiers_root),
            Value::from(self.gsrs_root),
        ])
    }

    /// Commitment of the state root array.
    pub fn hash(&self) -> Hash {
        self.array().commitment()
    }
}

/// Slot indices for the `StateRoot` record. Slot 0 is `_pad` (works
/// around pod2 issue #513); real fields start at slot 1.
pub const STATE_ROOT_BLOCK_NUMBER_SLOT: usize = 1;
pub const STATE_ROOT_TRANSACTIONS_SLOT: usize = 2;
pub const STATE_ROOT_NULLIFIERS_SLOT: usize = 3;
pub const STATE_ROOT_GSRS_SLOT: usize = 4;

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
    /// Construct from the source transaction's `ctx` dict (which carries
    /// the `live` set under the `"live"` key) plus the produced object
    /// whose membership we attest.
    pub fn new(ctx: &Dictionary, obj: &Dictionary) -> anyhow::Result<Self> {
        let tx_final = ctx.commitment();
        let (live_value, live_in_tx_proof) = ctx.prove(&StrKey::from("live"))?;
        let live = live_value
            .as_set()
            .ok_or_else(|| anyhow::anyhow!("ctx.live is not a Set"))?;
        let live_root = live.commitment();
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

/// Extract the `type` field from an object dict. The type is a
/// predicate hash that identifies the object's `IsX` rule.
pub fn object_type(obj: &Dictionary) -> Value {
    obj.get(&StrKey::from("type"))
        .expect("object dict lookup")
        .expect("object missing required type field")
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

/// Field name TxInsert's DictInsert clause stamps onto every newly
/// inserted object. Must stay in sync with `txlib.podlang`'s TxInsert
/// body and TxMutate's `Equal(old.identity, new.identity)` clause.
pub const IDENTITY_FIELD: &str = "identity";

/// Stamp `identity = commitment(initial)` into the dict and return the
/// materialized object. TxInsert's DictInsert clause proves the same
/// relationship; callers that need the post-identity dict outside of
/// `TxBuilder::insert` (e.g. tests, builders that pre-compute the
/// finalized object) should go through this helper to stay consistent.
pub fn with_identity(initial: &Dictionary) -> Dictionary {
    let identity = Value::from(initial.commitment());
    let mut new = initial.clone();
    new.insert(&StrKey::from(IDENTITY_FIELD), &identity)
        .unwrap();
    new
}

// ============================================================================
// Event tree (for replay construction in finalize)
// ============================================================================

pub(crate) enum ChainEvent {
    Insert {
        new: Dictionary,
        /// Pre-identity dict from which `new` was derived via
        /// `with_identity`. Threaded into replay so TxInsert's `initial`
        /// public arg (the dict the action constructed) can be bound at
        /// replay time.
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
const DISPLAY_SKIP_FIELDS: &[&str] = &["type", "key", IDENTITY_FIELD];

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
    /// `identity = commitment(initial)` via [`with_identity`] and the
    /// returned `Dictionary` is the post-identity `new` (the dict that
    /// lives in the tx). Subsequent mutate/delete must reference the
    /// returned dict, not `initial`.
    pub fn insert(
        &mut self,
        ctx: &mut BuildContext,
        initial: &Dictionary,
    ) -> (Dictionary, Statement, EventHandle) {
        assert!(
            !self.action_stack.is_empty(),
            "insert must be called inside an action scope",
        );
        let new = with_identity(initial);

        let prev = self.chain;
        let event_hash = hash_values(&[Value::from(EMPTY_VALUE), Value::from(new.clone())]);
        self.chain = hash_values(&[Value::from(prev), Value::from(event_hash)]);
        self.live.insert(&Value::from(new.clone())).unwrap();

        let new_type = object_type(&new);
        let st_dc = ctx
            .builder
            .priv_op(op!(DictContains(new, "type", new_type.clone())))
            .unwrap();
        let st_di = ctx
            .builder
            .priv_op(op!(DictInsert(new, initial, IDENTITY_FIELD, initial)))
            .unwrap();
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
                map!({"chain" => self.chain, "prev_chain" => prev, "initial" => initial.clone(), "new" => new.clone(), "type" => new_type}),
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
        let new_identity = new
            .get(&StrKey::from(IDENTITY_FIELD))
            .expect("new dict lookup")
            .expect("mutate target missing identity field (must come from TxBuilder::insert)");
        let old_identity = old
            .get(&StrKey::from(IDENTITY_FIELD))
            .expect("old dict lookup")
            .expect("mutate source missing identity field (must come from TxBuilder::insert)");
        assert_eq!(
            new_identity, old_identity,
            "mutate must preserve object identity"
        );
        let st_dc_new = ctx
            .builder
            .priv_op(op!(DictContains(new, "type", new_type.clone())))
            .unwrap();
        let st_dc_old = ctx
            .builder
            .priv_op(op!(DictContains(old, "type", new_type.clone())))
            .unwrap();
        let st_eq_identity = ctx
            .builder
            .priv_op(op!(Equal((old, IDENTITY_FIELD), (new, IDENTITY_FIELD))))
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
                map!({"chain" => self.chain, "prev_chain" => prev, "new" => new.clone(), "old" => old.clone(), "type" => new_type}),
                vec![st_dc_new, st_dc_old, st_eq_identity, st_h1, st_h2],
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
                map!({"chain" => self.chain, "prev_chain" => prev, "old" => old.clone(), "type" => old_type}),
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
        let state_root_arr = grounding.state_root.array();

        if inputs.is_empty() {
            // Base case: empty inputs. state_root is unconstrained here.
            let st = st_custom!(
                ctx,
                InputsGrounded(state_root = state_root_arr) = (
                    Equal(set!(), set!()),
                    Statement::None,
                    Statement::None,
                    Statement::None,
                    Statement::None
                )
            )
            .unwrap();
            record(&mut stats, "InputsGrounded");
            return (st, set!(), stats);
        }

        // One ArrayContains witness for `state_root.transactions`, reused
        // as the anchored-key arg for every per-input SetContains below.
        let (_, txns_proof) = state_root_arr
            .prove(STATE_ROOT_TRANSACTIONS_SLOT)
            .expect("state_root array has transactions slot");
        let st_state_root_transactions = ctx
            .builder
            .priv_op(Operation(
                OperationType::Native(NativeOperation::ArrayContainsFromEntries),
                vec![
                    Value::from(state_root_arr.commitment()).into(),
                    Value::from(STATE_ROOT_TRANSACTIONS_SLOT as i64).into(),
                    Value::from(grounding.state_root.transactions_root).into(),
                ],
                OperationAux::MerkleProof(txns_proof),
            ))
            .unwrap();

        let extend_set = |set: &Set, obj: &Dictionary| -> Set {
            let mut new_set = set.clone();
            new_set.insert(&Value::from(obj.clone())).unwrap();
            new_set
        };

        let prove_input = |ctx: &mut BuildContext,
                           evidence: &GroundingEvidence,
                           obj: &Dictionary|
         -> (Statement, Statement) {
            let st_tx_in_transactions = prove_source_tx_membership(
                ctx,
                &st_state_root_transactions,
                grounding,
                evidence.tx_final,
            );
            let st_set_contains_live = prove_obj_in_source_tx_live(ctx, evidence, obj);
            (st_tx_in_transactions, st_set_contains_live)
        };

        if inputs.len() == 1 {
            let (obj, evidence) = &inputs[0];
            let inputs_set = extend_set(&set!(), obj);
            let (st_tx_in_sr, st_set_contains_live) = prove_input(ctx, evidence, obj);
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
                InputsGrounded(state_root = state_root_arr) = (
                    Statement::None,
                    st_single,
                    Statement::None,
                    Statement::None,
                    Statement::None
                )
            )
            .unwrap();
            record(&mut stats, "InputsGrounded");
            return (st, inputs_set, stats);
        }

        // For 2+ inputs: bottom out at InputsGroundedPair (N=2) or
        // InputsGroundedTriple (N>=3), then peel any remaining inputs via
        // InputsGroundedRecursive.

        let (first_obj, first_evidence) = &inputs[0];
        let (second_obj, second_evidence) = &inputs[1];

        let set_first = extend_set(&set!(), first_obj);
        let (st_first_tx_in_sr, st_first_set_contains_live) =
            prove_input(ctx, first_evidence, first_obj);
        let st_igsv = st_custom!(
            ctx,
            InputsGroundedSingleVar() = (
                st_first_tx_in_sr,
                st_first_set_contains_live,
                SetInsert(set_first, set!(), first_obj)
            )
        )
        .unwrap();
        record(&mut stats, "InputsGroundedSingleVar");

        let inputs_pair = extend_set(&set_first, second_obj);
        let (st_second_tx_in_sr, st_second_set_contains_live) =
            prove_input(ctx, second_evidence, second_obj);

        if inputs.len() == 2 {
            let st_pair = st_custom!(
                ctx,
                InputsGroundedPair() = (
                    st_igsv,
                    st_second_tx_in_sr,
                    st_second_set_contains_live,
                    SetInsert(inputs_pair, set_first, second_obj)
                )
            )
            .unwrap();
            record(&mut stats, "InputsGroundedPair");
            let st = st_custom!(
                ctx,
                InputsGrounded(state_root = state_root_arr) = (
                    Statement::None,
                    Statement::None,
                    st_pair,
                    Statement::None,
                    Statement::None
                )
            )
            .unwrap();
            record(&mut stats, "InputsGrounded");
            return (st, inputs_pair, stats);
        }

        let st_igpv = st_custom!(
            ctx,
            InputsGroundedPairVar() = (
                st_igsv,
                st_second_tx_in_sr,
                st_second_set_contains_live,
                SetInsert(inputs_pair, set_first, second_obj)
            )
        )
        .unwrap();
        record(&mut stats, "InputsGroundedPairVar");

        let (third_obj, third_evidence) = &inputs[2];
        let inputs_triple = extend_set(&inputs_pair, third_obj);
        let (st_third_tx_in_sr, st_third_set_contains_live) =
            prove_input(ctx, third_evidence, third_obj);
        let st_triple = st_custom!(
            ctx,
            InputsGroundedTriple() = (
                st_igpv,
                st_third_tx_in_sr,
                st_third_set_contains_live,
                SetInsert(inputs_triple, inputs_pair, third_obj)
            )
        )
        .unwrap();
        record(&mut stats, "InputsGroundedTriple");

        let mut st = st_custom!(
            ctx,
            InputsGrounded(state_root = state_root_arr) = (
                Statement::None,
                Statement::None,
                Statement::None,
                st_triple,
                Statement::None
            )
        )
        .unwrap();
        record(&mut stats, "InputsGrounded");
        let mut prev_set = inputs_triple;

        for (obj, evidence) in &inputs[3..] {
            let next_set = extend_set(&prev_set, obj);
            let (st_tx_in_sr, st_set_contains_live) = prove_input(ctx, evidence, obj);
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
                InputsGrounded() = (
                    Statement::None,
                    Statement::None,
                    Statement::None,
                    Statement::None,
                    st_rec
                )
            )
            .unwrap();
            record(&mut stats, "InputsGrounded");
        }
        (st, prev_set, stats)
    }
}

/// Prove `SetContains(state_root.transactions, tx_hash)`. The supplied
/// statement must already witness `state_root["transactions"]` so we can
/// use it for the anchored key.
fn prove_source_tx_membership(
    ctx: &mut BuildContext,
    st_state_root_transactions: &Statement,
    grounding: &GroundingWitness,
    tx_hash: Hash,
) -> Statement {
    let proof = grounding
        .source_tx_proofs
        .get(&tx_hash)
        .cloned()
        .expect("missing source tx proof in grounding witness");
    ctx.builder
        .priv_op(Operation(
            OperationType::Native(NativeOperation::SetContainsFromEntries),
            vec![
                OperationArg::Statement(st_state_root_transactions.clone()),
                Value::from(tx_hash).into(),
            ],
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
    fn state_root_hash_matches_array_commitment() {
        let sr = StateRoot::new(7, test_hash(1), test_hash(2), test_hash(3));
        assert_eq!(sr.hash(), sr.array().commitment());
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

        let pick_initial = make_object(
            is_wood_pick.clone(),
            &[("durability", Value::from(100_i64))],
        );

        let mut tx1 = TxBuilder::new(&mut ctx, &[], state.grounding_witness(&[]));

        let scope = tx1.begin_action();
        let (pick, st_insert, h) = tx1.insert(&mut ctx, &pick_initial);
        let op_dur = ctx
            .builder
            .priv_op(op!(DictContains(pick, "durability", 100_i64)))
            .unwrap();
        let st_spawn = ctx
            .apply_custom_pred_simple(false, "SpawnWoodPick", vec![op_dur, st_insert])
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
        let stone_initial = make_object(is_stone.clone(), &[]);

        let witness = state.grounding_witness(std::slice::from_ref(&tx0));
        let evidence = GroundingEvidence::new(&tx0.ctx, &pick).unwrap();
        let inputs = vec![(pick.clone(), evidence)];
        let mut tx2 = TxBuilder::new(&mut ctx, &inputs, witness);

        let scope_outer = tx2.begin_action();

        // Sub-action: UseWoodPick (mutate pick)
        let st_use_wp = {
            let scope_sub = tx2.begin_action();
            let (st_mutate, h_sub) = tx2.mutate(&mut ctx, &pick_new, &pick);
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
                    vec![op_gt, op_sum, op_du, st_mutate],
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
        let (_stone, st_stone_insert, h) = tx2.insert(&mut ctx, &stone_initial);
        let st_mine = ctx
            .apply_custom_pred_simple(false, "MineStone", vec![st_use_wp, st_stone_insert])
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

        let log_initial = make_object(is_log.clone(), &[]);

        let mut tx1 = TxBuilder::new(&mut ctx, &[], state.grounding_witness(&[]));

        let scope = tx1.begin_action();
        let (log, st_insert, h) = tx1.insert(&mut ctx, &log_initial);
        let st_find = ctx
            .apply_custom_pred_simple(false, "FindLog", vec![st_insert])
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

        let wood_initial = make_object(is_wood.clone(), &[]);

        let witness = state.grounding_witness(std::slice::from_ref(&tx1_out));
        let evidence = GroundingEvidence::new(&tx1_out.ctx, &log).unwrap();
        let inputs = vec![(log.clone(), evidence)];
        let mut tx2 = TxBuilder::new(&mut ctx, &inputs, witness);

        let scope_outer = tx2.begin_action();

        // Sub-action: DeleteLog
        let st_del_log = {
            let scope_sub = tx2.begin_action();
            let (st_del, h_sub) = tx2.delete(&mut ctx, &log);
            let st_action = ctx
                .apply_custom_pred_simple(false, "DeleteLog", vec![st_del])
                .unwrap();
            let st_guard = ctx
                .apply_custom_pred_simple(false, "IsLog", vec![Statement::None, st_action.clone()])
                .unwrap();
            tx2.set_guard(h_sub, st_guard);
            tx2.end_action(scope_sub);
            st_action
        };

        // Direct: insert wood
        let (wood, st_ins, h) = tx2.insert(&mut ctx, &wood_initial);
        let st_craft_wood = ctx
            .apply_custom_pred_simple(false, "CraftWood", vec![st_del_log, st_ins])
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

        let stick_a_initial = make_object(is_stick.clone(), &[]);
        let stick_b_initial = make_object(is_stick, &[]);

        let witness = state.grounding_witness(std::slice::from_ref(&tx2_out));
        let evidence = GroundingEvidence::new(&tx2_out.ctx, &wood).unwrap();
        let inputs = vec![(wood.clone(), evidence)];
        let mut tx3 = TxBuilder::new(&mut ctx, &inputs, witness);

        let scope_outer = tx3.begin_action();

        // Sub-action: DeleteWood
        let st_del_wood = {
            let scope_sub = tx3.begin_action();
            let (st_del, h_sub) = tx3.delete(&mut ctx, &wood);
            let st_action = ctx
                .apply_custom_pred_simple(false, "DeleteWood", vec![st_del])
                .unwrap();
            let st_guard = ctx
                .apply_custom_pred_simple(false, "IsWood", vec![Statement::None, st_action.clone()])
                .unwrap();
            tx3.set_guard(h_sub, st_guard);
            tx3.end_action(scope_sub);
            st_action
        };

        // Direct: insert stick_a
        let (stick_a, st_ins_a, h_a) = tx3.insert(&mut ctx, &stick_a_initial);

        // Direct: insert stick_b
        let (stick_b, st_ins_b, h_b) = tx3.insert(&mut ctx, &stick_b_initial);

        // Pack stick_a / stick_b's pre-identity initials into an
        // `initials` dict so CraftSticks stays within the 8-wildcard
        // limit; rebind each TxInsert's slot 2 (initial) onto the
        // matching anchored key. TxInsert's arg layout is (chain,
        // prev_chain, initial, new, type).
        let initials = dict!({
            "stick_a" => stick_a_initial.clone(),
            "stick_b" => stick_b_initial.clone()
        });
        let st_ins_a_anchored = ctx
            .builder
            .priv_op(Operation::replace_value_with_entry(
                vec![None, None, Some((&initials, "stick_a")), None, None],
                st_ins_a,
            ))
            .unwrap();
        let st_ins_b_anchored = ctx
            .builder
            .priv_op(Operation::replace_value_with_entry(
                vec![None, None, Some((&initials, "stick_b")), None, None],
                st_ins_b,
            ))
            .unwrap();
        let st_craft_sticks = ctx
            .apply_custom_pred_simple(
                false,
                "CraftSticks",
                vec![st_del_wood, st_ins_a_anchored, st_ins_b_anchored],
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
