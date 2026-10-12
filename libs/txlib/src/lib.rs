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

use std::sync::LazyLock;
use std::{collections::HashMap, sync::Arc};

use anyhow::Context;
use pod2::{
    backends::plonky2::primitives::merkletree::MerkleProof,
    frontend::{Operation, entry},
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

/// Compact committed view of app state used for grounding transactions.
///
/// Holds only the Merkle roots needed to recompute the state
/// root hash and to verify synchronizer-supplied membership proofs. Full
/// containers are not carried -- callers prove each input's liveness with a
/// per-object Merkle proof packaged in a [`GroundingWitness`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StateHeader {
    /// Execution block number
    pub block_number: i64,
    /// Execution block timestamp
    pub block_timestamp: i64,
    /// Execution block hash
    pub block_hash: Hash,
    /// Root of the global created-object set: the commitment of every object
    /// state ever created. Grounding proves an input is a member here.
    pub created_root: Hash,
    pub nullifiers_root: Hash,
    pub prior_state_history_root: Hash,
}

impl StateHeader {
    pub fn new(
        block_number: i64,
        block_timestamp: i64,
        block_hash: Hash,
        created_root: Hash,
        nullifiers_root: Hash,
        prior_state_history_root: Hash,
    ) -> Self {
        Self {
            block_number,
            block_timestamp,
            block_hash,
            created_root,
            nullifiers_root,
            prior_state_history_root,
        }
    }

    /// Array view used as the state root record. Slot layout
    /// matches the `record StateHeader` declaration in txlib.podlang.
    /// Predicates access fields via anchored-key syntax (e.g.
    /// `state_header.created`).
    pub fn array(&self) -> Array {
        Array::new(vec![
            Value::from(self.block_number),
            Value::from(self.block_timestamp),
            Value::from(self.block_hash),
            Value::from(self.created_root),
            Value::from(self.nullifiers_root),
            Value::from(self.prior_state_history_root),
        ])
    }

    /// Commitment of the state root array.
    pub fn hash(&self) -> Hash {
        self.array().commitment()
    }
}

/// Slot indices for the `StateHeader` record, matching the field order in
/// the `record StateHeader` declaration in txlib.podlang.
pub const STATE_HEADER_BLOCK_NUMBER_SLOT: usize = 0;
pub const STATE_HEADER_BLOCK_TIMESTAMP_SLOT: usize = 1;
pub const STATE_HEADER_BLOCK_HASH_SLOT: usize = 2;
pub const STATE_HEADER_CREATED_SLOT: usize = 3;
pub const STATE_HEADER_NULLIFIERS_SLOT: usize = 4;
pub const STATE_HEADER_PRIOR_STATE_HISTORY_SLOT: usize = 5;

pub static RECORD_STATE_HEADER_FIELDS: LazyLock<Arc<Vec<String>>> = LazyLock::new(|| {
    Arc::new(
        [
            "block_number",
            "block_timestamp",
            "block_hash",
            "created",
            "nullifiers",
            "prior_state_history",
        ]
        .map(|s| s.to_string())
        .into_iter()
        .collect::<Vec<_>>(),
    )
});

pub static RECORD_STATE_HEADER_PODLANG: LazyLock<String> = LazyLock::new(|| {
    let mut s = "record StateHeader = (".to_string();
    for (i, f) in RECORD_STATE_HEADER_FIELDS.iter().enumerate() {
        if i != 0 {
            s += ", ";
        }
        s += f;
    }
    s += ")";
    s
});

/// Proof-bearing grounding data required to build a new transaction.
///
/// Callers use `state_header` as the committed global context and
/// `created_proofs` to prove that each consumed input object is present in
/// `state_header.created_root` (the global created-object set). Proofs are keyed
/// by object commitment (`Dictionary::commitment()`) and carry the object's
/// array index, since grounding is `ArrayContains(created, index, obj)`. They
/// are fetched fresh at consume time because the created set grows: a proof is
/// only valid against the state root it was drawn from.
#[derive(Clone, Debug)]
pub struct GroundingWitness {
    pub state_header: StateHeader,
    /// Per-object `(index, Merkle proof)` for membership in the global created
    /// set, keyed by object commitment (`Dictionary::commitment()`).
    pub created_proofs: HashMap<Hash, (i64, MerkleProof)>,
}

impl GroundingWitness {
    pub fn new(
        state_header: StateHeader,
        created_proofs: HashMap<Hash, (i64, MerkleProof)>,
    ) -> Self {
        Self {
            state_header,
            created_proofs,
        }
    }
}

/// Output of a finalized transaction. Created states and nullifiers are public
/// proof outputs; live states are retained for local bookkeeping.
#[derive(Clone, Debug)]
pub struct Tx {
    pub live: Set,
    pub created: Set,
    pub nullifiers: Set,
    /// The public final dictionary. Its commitment is tx_final (the value the
    /// relayer publishes). Contains created, nullifiers, chain_start, chain_end.
    /// Private inputs are removed before computing this commitment.
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

    /// Commitments of all states produced, including subsequently consumed states.
    pub fn created_commitments(&self) -> anyhow::Result<Vec<Hash>> {
        self.created
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
    created: Set,
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
            created: self.created.clone(),
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
            created: payload.created,
            nullifiers: payload.nullifiers,
            ctx: payload.ctx,
            state_header: Arc::new(payload.state_header),
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
    Dictionary::new(map)
}

/// Field name TxInsert's DictInsert clause stamps onto every newly
/// inserted object. Must stay in sync with `txlib.podlang`'s TxInsert
/// body and TxMutate's `Equal(old.stable_identifier, new.stable_identifier)`
/// clause.
pub const STABLE_IDENTIFIER_FIELD: &str = "stable_identifier";

/// Stamp `stable_identifier = commitment(initial)` into the dict and
/// return the materialized object. TxInsert's DictInsert clause proves
/// the same relationship; callers that need the post-identity dict
/// outside of `TxBuilder::insert` (e.g. tests, builders that pre-compute
/// the finalized object) should go through this helper to stay consistent.
pub fn with_stable_identifier(initial: &Dictionary) -> Dictionary {
    let stable_identifier = Value::from(initial.commitment());
    let mut new = initial.clone();
    new.insert(&StrKey::from(STABLE_IDENTIFIER_FIELD), &stable_identifier)
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

/// Build a replay tx dict (the event chain is separate).
pub(crate) fn build_tx(
    inputs: &Set,
    created: &Set,
    nullifiers: &Set,
    chain_start: Hash,
    chain_end: Hash,
) -> Dictionary {
    dict!({
        "inputs" => inputs.clone(),
        "created" => created.clone(),
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

struct GroundedInput {
    object: Dictionary,
    membership: Statement,
}

pub struct TxBuilder {
    pub chain: Hash,
    pub chain_start: Hash,
    live: Set,
    nullifiers: Set,
    state_header: Arc<StateHeader>,
    grounded_inputs: Vec<GroundedInput>,
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
        let mut inputs_set = set!();
        let grounded_inputs = inputs
            .iter()
            .map(|object| {
                inputs_set.insert(&Value::from(object.clone())).unwrap();
                GroundedInput {
                    object: object.clone(),
                    membership: prove_obj_in_created(
                        ctx,
                        grounding.state_header.created_root,
                        &grounding,
                        object,
                    ),
                }
            })
            .collect();
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
            grounded_inputs,
            inputs_set,
            events: vec![],
            action_stack: vec![],
            next_scope_id: 0,
            stats: TxStats::new(),
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
                entry(old, STABLE_IDENTIFIER_FIELD).expect("referenced container entry exists"),
                entry(new, STABLE_IDENTIFIER_FIELD).expect("referenced container entry exists")
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
    /// Returns an error if an input is not spent or an object state is
    /// created more than once.
    pub fn finalize(self, ctx: &mut BuildContext) -> anyhow::Result<(Statement, Tx, TxStats)> {
        assert!(self.action_stack.is_empty(), "unclosed action scopes");
        assert!(
            !self.events.is_empty(),
            "finalize: Tx must contain at least one top-level action"
        );

        let mut stats = self.stats;
        let zero: Hash = EMPTY_VALUE.into();

        let empty_created = set!();
        let before_tx = build_tx(&self.inputs_set, &empty_created, &set!(), zero, zero);

        // Replay the top-level action sequence. Every top-level event
        // is guaranteed to be a ChainEvent::Action (enforced by the
        // begin_action/end_action API), so we dispatch directly to
        // ReplayActions instead of going through ReplayContents.
        let empty_nullifiers = set!();
        let frame = replay::ReplayFrame {
            inputs: &self.inputs_set,
            created: &empty_created,
            nullifiers: &empty_nullifiers,
            chain_start: zero,
            chain_end: zero,
        };
        let (st_replay, _, created, replay_nullifiers) = replay::Replayer::new(ctx, &mut stats)
            .build_replay_actions(&self.events, self.chain_start, frame)?;

        let (st_inputs_grounded, _) = Self::build_inputs_grounded_and_spent(
            ctx,
            &self.grounded_inputs,
            self.state_header.created_root,
            &self.nullifiers,
            &mut stats,
        )?;

        assert_eq!(replay_nullifiers.commitment(), self.nullifiers.commitment());
        let after_tx = build_tx(&self.inputs_set, &created, &self.nullifiers, zero, zero);
        let mut tx_final = after_tx.clone();
        tx_final.delete(&StrKey::from("inputs"))?;

        // Tie grounding to the public state root: rebind InputsGroundedAndSpent
        // statement's `inputs` to `before_tx.inputs` and
        // its created set to `state_header.created`. The latter is the single
        // state-root array access anchoring the whole grounding tree. Two calls
        // rather than one because the entries are different anchored-key types:
        // a dict key and an array index.
        let st_inputs_rebound = ctx
            .builder
            .priv_op(Operation::replace_value_with_entry(
                vec![
                    Some(entry(&before_tx, "inputs").expect("referenced container entry exists")),
                    None,
                    None,
                ],
                st_inputs_grounded,
            ))
            .unwrap();
        let state_header_arr = self.state_header.array();
        let st_inputs_rebound = ctx
            .builder
            .priv_op(Operation::replace_value_with_entry(
                vec![
                    None,
                    Some(
                        entry(&state_header_arr, STATE_HEADER_CREATED_SLOT as i64)
                            .expect("referenced container entry exists"),
                    ),
                    None,
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
                vec![
                    Some(entry(&before_tx, "inputs").expect("referenced container entry exists")),
                    None,
                    None,
                ],
                st_hash,
            ))
            .unwrap();
        // Pin the full schema of `before_tx` (nullifiers={}, chain_start={},
        // chain_end={}, created={}, inputs=inputs_set) in a single DictInsert clause. This
        // closes the malleability where the prover could otherwise witness
        // arbitrary chain_start/chain_end values that pass through ReplayActions
        // verbatim into tx_final.
        let scope_dict = dict!({
            "created" => set!(),
            "nullifiers" => set!(),
            "chain_start" => zero,
            "chain_end" => zero
        });
        let st_dict_insert_lit = ctx
            .builder
            .priv_op(op!(DictInsert(
                scope_dict,
                "inputs",
                self.inputs_set,
                before_tx
            )))
            .unwrap();
        let st_dict_insert = ctx
            .builder
            .priv_op(Operation::replace_value_with_entry(
                vec![
                    None,
                    None,
                    Some(entry(&before_tx, "inputs").expect("referenced container entry exists")),
                    None,
                ],
                st_dict_insert_lit,
            ))
            .unwrap();
        // Surface the final nullifier and created sets as public args.
        let st_remove_inputs = ctx
            .builder
            .priv_op(op!(DictDelete(after_tx, "inputs", tx_final)))?;
        let st_dc_null_after = ctx
            .builder
            .priv_op(op!(DictContains(tx_final, "nullifiers", self.nullifiers)))
            .unwrap();
        let st_dc_created_after = ctx
            .builder
            .priv_op(op!(DictContains(tx_final, "created", created)))
            .unwrap();
        let st_bindings = ctx
            .apply_custom_pred_simple(
                false,
                "TxFinalBindings",
                vec![st_remove_inputs, st_dc_null_after, st_dc_created_after],
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
            created,
            nullifiers: self.nullifiers,
            ctx: tx_final,
            state_header: self.state_header,
        };
        Ok((st, tx, stats))
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

    fn build_inputs_grounded_and_spent(
        ctx: &mut BuildContext,
        inputs: &[GroundedInput],
        created_root: Hash,
        nullifiers: &Set,
        stats: &mut TxStats,
    ) -> anyhow::Result<(Statement, Set)> {
        let mut branches = vec![Statement::None; 5];
        let mut inputs_set = set!();
        if inputs.is_empty() {
            branches[0] = ctx.builder.priv_op(op!(Equal(set!(), set!())))?;
        } else {
            let split = inputs.len().saturating_sub(4);
            let previous = if inputs.len() >= 4 {
                let (statement, previous_set) = Self::build_inputs_grounded_and_spent(
                    ctx,
                    &inputs[..split],
                    created_root,
                    nullifiers,
                    stats,
                )?;
                inputs_set = previous_set;
                Some(statement)
            } else {
                None
            };
            let mut statements = Vec::new();
            for input in &inputs[split..] {
                let object = &input.object;
                let key_hash = object_key_hash(object)?;
                let nullifier = object_nullifier_from_key_hash(key_hash);
                let spent = ctx
                    .builder
                    .priv_op(op!(SetContains(nullifiers, nullifier)))
                    .with_context(|| {
                        format!("input object {} was not spent", object.commitment())
                    })?;
                let mut next_set = inputs_set.clone();
                next_set.insert(&Value::from(object.clone()))?;
                // The builder deduplicates these hashes against ReplayNullify.
                let statement = st_custom!(
                    ctx,
                    InputGroundedAndSpent() = (
                        input.membership.clone(),
                        SetInsert(inputs_set, object, next_set),
                        Hash(object, entry(object, "key")?, key_hash),
                        Hash(key_hash, OBJECT_NULLIFIER_VERSION, nullifier),
                        spent
                    )
                )?;
                record(stats, "InputGroundedAndSpent");
                statements.push(statement);
                inputs_set = next_set;
            }
            let branch = inputs.len().min(4);
            let statement = if branch == 1 {
                statements.remove(0)
            } else {
                let name = match branch {
                    2 => "InputsGroundedAndSpentPair",
                    3 => "InputsGroundedAndSpentTriple",
                    4 => {
                        statements.push(previous.expect("recursive grounding has a prefix"));
                        "InputsGroundedAndSpentRecursive"
                    }
                    _ => unreachable!(),
                };
                let statement = ctx.apply_custom_pred_simple(false, name, statements)?;
                record(stats, name);
                statement
            };
            branches[branch] = statement;
        }
        let statement = ctx.apply_custom_pred(
            false,
            "InputsGroundedAndSpent",
            map!({"created" => Value::from(created_root), "nullifiers" => nullifiers.clone()}),
            branches,
        )?;
        record(stats, "InputsGroundedAndSpent");
        Ok((statement, inputs_set))
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

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use hex::FromHex;
    use pod2::{
        backends::plonky2::mock::mainpod::MockProver,
        frontend::{MainPod, MultiPodBuilder},
        middleware::{F, Params, Predicate, VDSet, containers::Array},
    };
    use pod2utils::{macros::BuildContext, set};

    use super::*;

    /// Running grounding state for the tests: keeps the full created-object set
    /// (as an array, plus a reverse index for proofs) and the nullifier set so
    /// it can hand out real Merkle proofs, while exposing only the
    /// commitments-only `StateHeader`. The created set is grow-only.
    struct TestState {
        block_number: i64,
        block_timestamp: i64,
        block_hash: Hash,
        created: Array,
        created_index: HashMap<Hash, i64>,
        nullifiers: Set,
        state_history: Array,
    }

    impl TestState {
        fn empty(block_number: i64) -> Self {
            Self {
                block_number,
                block_timestamp: block_number * 1000,
                block_hash: Hash([F(0), F(0), F(0), F(block_number as u64)]),
                created: Array::new(Vec::new()),
                created_index: HashMap::new(),
                nullifiers: set!(),
                state_history: Array::new(Vec::new()),
            }
        }

        fn state_header(&self) -> StateHeader {
            StateHeader::new(
                self.block_number,
                self.block_timestamp,
                self.block_hash,
                self.created.commitment(),
                self.nullifiers.commitment(),
                self.state_history.commitment(),
            )
        }

        fn apply_tx(&mut self, tx: &Tx) {
            for obj in tx.created.iter() {
                let obj = obj.expect("tx created entry should decode");
                let commitment = Hash(obj.raw().0);
                let index = self.created_index.len() as i64;
                self.created.insert(index as usize, obj).unwrap();
                self.created_index.insert(commitment, index);
            }
            for nullifier in tx.nullifiers.iter() {
                let nullifier = nullifier.expect("tx nullifier should decode");
                self.nullifiers.insert(&nullifier).unwrap();
            }
        }

        /// Build a grounding witness for the given input objects: one created-set
        /// `(index, membership proof)` per object, keyed by commitment.
        fn grounding_witness(&self, inputs: &[Dictionary]) -> Arc<GroundingWitness> {
            let created_proofs = inputs
                .iter()
                .map(|obj| {
                    let commitment = obj.commitment();
                    let index = *self
                        .created_index
                        .get(&commitment)
                        .expect("input object should be present in created set");
                    let (_value, proof) = self
                        .created
                        .prove(index as usize)
                        .expect("input object should be provable from created set");
                    (commitment, (index, proof))
                })
                .collect();
            Arc::new(GroundingWitness::new(self.state_header(), created_proofs))
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
        assert_eq!(obj.iter().count(), 1); // key
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
    fn state_header_hash_matches_array_commitment() {
        let sr = StateHeader::new(7, 8, test_hash(4), test_hash(1), test_hash(2), test_hash(3));
        assert_eq!(sr.hash(), sr.array().commitment());
    }

    #[test]
    fn state_header_serializes_and_deserializes_camelcase() {
        let original = StateHeader::new(
            9,
            10,
            test_hash(5),
            test_hash(1),
            test_hash(2),
            test_hash(3),
        );
        let encoded = serde_json::to_value(&original).unwrap();
        assert_eq!(encoded["blockNumber"], serde_json::json!(9));
        assert_eq!(encoded["blockTimestamp"], serde_json::json!(10));
        assert_eq!(
            encoded["blockHash"],
            serde_json::json!(hex::encode([5_u8; 32]))
        );
        assert_eq!(
            encoded["createdRoot"],
            serde_json::json!(hex::encode([1_u8; 32]))
        );
        assert_eq!(
            encoded["nullifiersRoot"],
            serde_json::json!(hex::encode([2_u8; 32]))
        );
        assert_eq!(
            encoded["priorStateHistoryRoot"],
            serde_json::json!(hex::encode([3_u8; 32]))
        );

        let decoded: StateHeader = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded, original);
    }

    fn replay_test_context() -> (BuildContext, Value) {
        let params = Params::default();
        let vd_set = VDSet::new(&[]);
        // An unrestricted test class isolates replay from application authorization.
        let guard_module = Arc::new(
            pod2::lang::load_module(
                "ReplayTestGuard(obj, state_header, chain_start, chain_end) = AND(Equal(obj, obj))",
                "replay_test",
                &params,
                &[],
            )
            .unwrap(),
        );
        let guard_type = Value::from(
            Predicate::Custom(
                guard_module
                    .predicate_ref_by_name("ReplayTestGuard")
                    .unwrap(),
            )
            .hash(),
        );
        let ctx = BuildContext::new(
            MultiPodBuilder::new(&params, &vd_set),
            vec![
                Arc::new(predicates::events_module()),
                Arc::new(predicates::module()),
                guard_module,
            ],
        );
        (ctx, guard_type)
    }

    fn attach_guard(
        ctx: &mut BuildContext,
        builder: &mut TxBuilder,
        object: &Dictionary,
        handle: EventHandle,
        chain_start: Hash,
    ) {
        let equal = ctx.builder.priv_op(op!(Equal(object, object))).unwrap();
        let guard = ctx
            .apply_custom_pred(
                false,
                "ReplayTestGuard",
                map!({
                    "state_header" => builder.state_header().array(),
                    "chain_start" => chain_start,
                    "chain_end" => builder.chain_position()
                }),
                vec![equal],
            )
            .unwrap();
        builder.set_guard(handle, guard);
    }

    fn grounded_replay_inputs(
        state: &mut TestState,
        guard_type: Value,
        count: usize,
    ) -> Vec<Dictionary> {
        (0..count)
            .map(|index| {
                let object = with_stable_identifier(&dict!({
                    "key" => index as i64 + 100,
                    "type" => guard_type.clone(),
                    "version" => 0_i64
                }));
                state
                    .created
                    .insert(index, Value::from(object.clone()))
                    .unwrap();
                state
                    .created_index
                    .insert(object.commitment(), index as i64);
                object
            })
            .collect()
    }

    #[test]
    fn grounds_and_spends_inputs_across_recursive_boundaries() {
        for count in (0..=9).chain([16]) {
            let (mut ctx, guard_type) = replay_test_context();
            let mut state = TestState::empty(0);
            let inputs = grounded_replay_inputs(&mut state, guard_type.clone(), count);
            let mut builder = TxBuilder::new(&mut ctx, &inputs, state.grounding_witness(&inputs));
            let outer_start = builder.chain_position();
            let outer = builder.begin_action();
            if inputs.is_empty() {
                let (object, _, handle) = builder.insert(&mut ctx, &make_object(guard_type, &[]));
                attach_guard(&mut ctx, &mut builder, &object, handle, outer_start);
            }
            for (index, input) in inputs.iter().enumerate() {
                let start = builder.chain_position();
                let child = builder.begin_action();
                if index % 2 == 0 {
                    let output = tx_with(input, "version", Value::from(1_i64));
                    let (_, handle) = builder.mutate(&mut ctx, &output, input);
                    attach_guard(&mut ctx, &mut builder, &output, handle, start);
                } else {
                    let (_, handle) = builder.delete(&mut ctx, input);
                    attach_guard(&mut ctx, &mut builder, input, handle, start);
                }
                builder.end_action(child);
            }
            builder.end_action(outer);
            let (statement, tx, stats) = builder.finalize(&mut ctx).unwrap();
            assert_eq!(
                stats.get("InputGroundedAndSpent").copied().unwrap_or(0),
                count
            );
            assert_eq!(
                stats
                    .get("InputsGroundedAndSpentRecursive")
                    .copied()
                    .unwrap_or(0),
                count / 4
            );
            assert_eq!(stats["InputsGroundedAndSpent"], count / 4 + 1);
            for input in &inputs {
                let key_hash = object_key_hash(input).unwrap();
                let nullifier = object_nullifier_from_key_hash(key_hash);
                assert!(tx.nullifiers.contains(&Value::from(nullifier)).unwrap());
                for hash in [key_hash, nullifier] {
                    let occurrences = ctx
                        .builder
                        .statements()
                        .iter()
                        .filter(|statement| {
                            matches!(statement, Statement::Hash(..))
                                && statement.args()[2]
                                    .literal()
                                    .is_ok_and(|value| value.raw() == Value::from(hash).raw())
                        })
                        .count();
                    assert_eq!(occurrences, 1, "grounding must reuse replay's hashes");
                }
            }
            ctx.builder.reveal(&statement).unwrap();
            solve_and_verify(ctx.builder);
        }
    }

    #[test]
    fn grounding_rejects_membership_in_a_different_nullifier_set() {
        let (mut ctx, guard_type) = replay_test_context();
        let mut state = TestState::empty(0);
        let inputs = grounded_replay_inputs(&mut state, guard_type, 1);
        let input = &inputs[0];
        let witness = state.grounding_witness(&inputs);
        let membership =
            prove_obj_in_created(&mut ctx, state.created.commitment(), &witness, input);
        let empty = set!();
        let input_set = set!(input);
        let key_hash = object_key_hash(input).unwrap();
        let nullifier = object_nullifier_from_key_hash(key_hash);
        let nullifiers = set!(nullifier);
        let insert = ctx
            .builder
            .priv_op(op!(SetInsert(empty, input, input_set)))
            .unwrap();
        let hash_key = ctx
            .builder
            .priv_op(op!(Hash(input, entry(input, "key").unwrap(), key_hash)))
            .unwrap();
        let hash_nullifier = ctx
            .builder
            .priv_op(op!(Hash(key_hash, OBJECT_NULLIFIER_VERSION, nullifier)))
            .unwrap();
        let spent = ctx
            .builder
            .priv_op(op!(SetContains(nullifiers, nullifier)))
            .unwrap();
        let premises = vec![membership, insert, hash_key, hash_nullifier, spent];
        let predicate = ctx.modules[1]
            .predicate_ref_by_name("InputGroundedAndSpent")
            .unwrap();
        let arguments = vec![
            (0, Value::from(empty.clone())),
            (1, Value::from(input_set)),
            (2, Value::from(state.created.commitment())),
            (3, Value::from(nullifiers)),
        ];
        ctx.builder
            .op(
                false,
                arguments.clone(),
                Operation::custom(predicate.clone(), premises.clone()),
            )
            .unwrap();
        let mut arguments = arguments;
        arguments[3].1 = Value::from(empty);
        assert!(
            ctx.builder
                .op(false, arguments, Operation::custom(predicate, premises))
                .is_err()
        );
    }

    #[test]
    fn rejects_unspent_inputs_including_stale_grounding() {
        for count in [1, 2, 3, 4, 5, 8] {
            for unspent_index in [0, count - 1] {
                for stale in [false, true] {
                    let (mut ctx, guard_type) = replay_test_context();
                    let mut state = TestState::empty(0);
                    let inputs = grounded_replay_inputs(&mut state, guard_type.clone(), count);
                    let witness = state.grounding_witness(&inputs);
                    let nullifier = compute_nullifier(&inputs[unspent_index]);
                    state.nullifiers.insert(&Value::from(nullifier)).unwrap();
                    let witness = if stale {
                        witness
                    } else {
                        state.grounding_witness(&inputs)
                    };
                    let mut builder = TxBuilder::new(&mut ctx, &inputs, witness);
                    let start = builder.chain_position();
                    let scope = builder.begin_action();
                    let (output, _, output_handle) =
                        builder.insert(&mut ctx, &make_object(guard_type, &[]));
                    let mut guards = vec![(output, output_handle)];
                    for (index, input) in inputs.iter().enumerate() {
                        if index != unspent_index {
                            let (_, handle) = builder.delete(&mut ctx, input);
                            guards.push((input.clone(), handle));
                        }
                    }
                    for (object, handle) in guards {
                        attach_guard(&mut ctx, &mut builder, &object, handle, start);
                    }
                    builder.end_action(scope);
                    let error = builder.finalize(&mut ctx).unwrap_err();
                    assert_eq!(
                        error.to_string(),
                        format!(
                            "input object {} was not spent",
                            inputs[unspent_index].commitment()
                        )
                    );
                }
            }
        }
    }

    #[test]
    fn publishes_intermediate_and_deleted_states() {
        let (mut ctx, guard_type) = replay_test_context();
        let input = with_stable_identifier(&make_object(guard_type.clone(), &[]));
        let mut state = TestState::empty(0);
        state.created.insert(0, Value::from(input.clone())).unwrap();
        state.created_index.insert(input.commitment(), 0);
        let inputs = vec![input.clone()];
        let mut builder = TxBuilder::new(&mut ctx, &inputs, state.grounding_witness(&inputs));

        let outer_start = builder.chain_position();
        let outer = builder.begin_action();
        let (_, input_handle) = builder.delete(&mut ctx, &input);
        let initial = make_object(guard_type.clone(), &[("version", Value::from(0_i64))]);
        let (first, _, first_handle) = builder.insert(&mut ctx, &initial);
        let child_start = builder.chain_position();
        let child = builder.begin_action();
        let mut second = tx_with(&first, "version", Value::from(1_i64));
        rekey(&mut second);
        let (_, second_handle) = builder.mutate(&mut ctx, &second, &first);
        let mut third = tx_with(&second, "version", Value::from(2_i64));
        rekey(&mut third);
        let (_, third_handle) = builder.mutate(&mut ctx, &third, &second);
        let (_, delete_handle) = builder.delete(&mut ctx, &third);
        for (object, handle) in [
            (&second, second_handle),
            (&third, third_handle),
            (&third, delete_handle),
        ] {
            attach_guard(&mut ctx, &mut builder, object, handle, child_start);
        }
        builder.end_action(child);
        let survivor_initial = make_object(guard_type, &[("version", Value::from(0_i64))]);
        let (survivor, _, survivor_handle) = builder.insert(&mut ctx, &survivor_initial);
        attach_guard(&mut ctx, &mut builder, &input, input_handle, outer_start);
        attach_guard(&mut ctx, &mut builder, &first, first_handle, outer_start);
        attach_guard(
            &mut ctx,
            &mut builder,
            &survivor,
            survivor_handle,
            outer_start,
        );
        builder.end_action(outer);

        let next_start = builder.chain_position();
        let next = builder.begin_action();
        let mut final_state = tx_with(&survivor, "version", Value::from(1_i64));
        rekey(&mut final_state);
        let (_, handle) = builder.mutate(&mut ctx, &final_state, &survivor);
        attach_guard(&mut ctx, &mut builder, &final_state, handle, next_start);
        builder.end_action(next);

        let (statement, tx, stats) = builder.finalize(&mut ctx).unwrap();
        let expected_created = set!(first, second, third, survivor, final_state);
        assert_eq!(tx.created.commitment(), expected_created.commitment());
        assert_eq!(tx.live.commitment(), set!(final_state).commitment());
        assert_eq!(
            statement.args()[3].literal().unwrap(),
            Value::from(tx.created.clone())
        );
        // An observer can reconstruct tx_final from public data alone. Private
        // inputs must not enter this commitment, even though they enter replay.
        let public_final = dict!({
            "created" => tx.created.commitment(),
            "nullifiers" => tx.nullifiers.commitment(),
            "chain_start" => EMPTY_VALUE,
            "chain_end" => EMPTY_VALUE
        });
        assert_eq!(tx.dict().commitment(), public_final.commitment());
        assert!(tx.dict().get(&StrKey::from("inputs")).unwrap().is_none());
        assert_eq!(
            statement.args()[1].literal().unwrap(),
            Value::from(public_final)
        );
        assert_eq!(tx.created_commitments().unwrap().len(), 5);
        assert_eq!(tx.nullifier_hashes().unwrap().len(), 5);
        assert_eq!(stats["ReplayMutateEvent"], 3);
        assert_eq!(stats["ReplayNullify"], 5);
        assert_eq!(stats["ReplayDelete"], 2);
        ctx.builder.reveal(&statement).unwrap();
        solve_and_verify(ctx.builder);

        let decoded: Tx = serde_json::from_str(&serde_json::to_string(&tx).unwrap()).unwrap();
        assert_eq!(decoded.created.commitment(), tx.created.commitment());
        assert_eq!(decoded.live.commitment(), tx.live.commitment());
        assert_eq!(decoded.ctx.commitment(), tx.ctx.commitment());
        state.apply_tx(&tx);
        for object in [&first, &second, &third, &survivor, &final_state] {
            assert!(state.created_index.contains_key(&object.commitment()));
        }
    }

    #[test]
    fn recreating_a_state_returns_an_error() {
        // Both mutation cycles and delete/reinsert must fail without unwinding,
        // including when the previous creation was in a parent action scope.
        for mutate_cycle in [true, false] {
            let (mut ctx, guard_type) = replay_test_context();
            let state = TestState::empty(0);
            let mut builder = TxBuilder::new(&mut ctx, &[], state.grounding_witness(&[]));
            let outer_start = builder.chain_position();
            let outer = builder.begin_action();
            let initial = make_object(guard_type, &[("version", Value::from(0_i64))]);
            let (first, _, first_handle) = builder.insert(&mut ctx, &initial);
            let child_start = builder.chain_position();
            let child = builder.begin_action();
            let (first_child_obj, first_child_handle, repeated_handle) = if mutate_cycle {
                let mut second = tx_with(&first, "version", Value::from(1_i64));
                rekey(&mut second);
                let (_, second_handle) = builder.mutate(&mut ctx, &second, &first);
                let (_, repeated_handle) = builder.mutate(&mut ctx, &first, &second);
                (second, second_handle, repeated_handle)
            } else {
                let (_, delete_handle) = builder.delete(&mut ctx, &first);
                let (repeated, _, repeated_handle) = builder.insert(&mut ctx, &initial);
                assert_eq!(repeated.commitment(), first.commitment());
                (first.clone(), delete_handle, repeated_handle)
            };
            attach_guard(
                &mut ctx,
                &mut builder,
                &first_child_obj,
                first_child_handle,
                child_start,
            );
            attach_guard(&mut ctx, &mut builder, &first, repeated_handle, child_start);
            builder.end_action(child);
            attach_guard(&mut ctx, &mut builder, &first, first_handle, outer_start);
            builder.end_action(outer);

            let err = builder.finalize(&mut ctx).unwrap_err();
            assert_eq!(
                err.to_string(),
                format!(
                    "object state {} was already created in this transaction",
                    first.commitment()
                )
            );
        }
    }

    /// Tx 1: Spawn a WoodPick (insert, no inputs).
    /// Tx 2: MineStone using the WoodPick (mutate pick + insert stone).
    #[test]
    fn test_mine_stone() {
        let events = Arc::new(crate::predicates::events_module());
        let txlib = Arc::new(crate::predicates::module());
        let craft = Arc::new(crate::predicates::crafting_test_module());
        let modules = vec![events, txlib.clone(), craft.clone()];

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
            .apply_custom_pred(
                false,
                "IsWoodPick",
                map!({"state_header" => state.state_header().array()}),
                vec![st_spawn.clone(), Statement::None, Statement::None],
            )
            .unwrap();
        tx1.set_guard(h, st_guard);
        tx1.end_action(scope);

        eprintln!("{tx1}");
        let (st, tx0, stats) = tx1.finalize(&mut ctx).unwrap();
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

        let inputs = vec![pick.clone()];
        let witness = state.grounding_witness(&inputs);
        let mut tx2 = TxBuilder::new(&mut ctx, &inputs, witness);

        let scope_outer = tx2.begin_action();

        // Sub-action: UseWoodPick (mutate pick)
        let st_use_wp = {
            let scope_sub = tx2.begin_action();
            let (st_mutate, h_sub) = tx2.mutate(&mut ctx, &pick_new, &pick);
            let op_gt = ctx
                .builder
                .priv_op(op!(Gt(
                    entry(&pick, "durability").expect("referenced container entry exists"),
                    0_i64
                )))
                .unwrap();
            let op_sum = ctx
                .builder
                .priv_op(op!(Sum(
                    99_i64,
                    1_i64,
                    entry(&pick, "durability").expect("referenced container entry exists")
                )))
                .unwrap();
            let op_du = ctx
                .builder
                .priv_op(op!(DictUpdate(pick, "durability", 99_i64, pick_new)))
                .unwrap();
            let st_action = ctx
                .apply_custom_pred_simple(
                    false,
                    "UseWoodPick",
                    vec![op_gt, op_sum, op_du, st_mutate],
                )
                .unwrap();
            let st_guard = ctx
                .apply_custom_pred(
                    false,
                    "IsWoodPick",
                    map!({"state_header" => state.state_header().array()}),
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
            .apply_custom_pred(
                false,
                "IsStone",
                map!({"state_header" => state.state_header().array()}),
                vec![st_mine.clone()],
            )
            .unwrap();
        tx2.set_guard(h, st_guard);
        tx2.end_action(scope_outer);

        eprintln!("{tx2}");
        let (st, tx_out, stats) = tx2.finalize(&mut ctx).unwrap();
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
        let events = Arc::new(crate::predicates::events_module());
        let txlib = Arc::new(crate::predicates::module());
        let craft = Arc::new(crate::predicates::crafting_test_module());
        let modules = vec![events, txlib.clone(), craft.clone()];

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
            .apply_custom_pred(
                false,
                "IsLog",
                map!({"state_header" => state.state_header().array()}),
                vec![st_find.clone(), Statement::None],
            )
            .unwrap();
        tx1.set_guard(h, st_guard);
        tx1.end_action(scope);

        eprintln!("{tx1}");
        let (st, tx1_out, stats) = tx1.finalize(&mut ctx).unwrap();
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

        let inputs = vec![log.clone()];
        let witness = state.grounding_witness(&inputs);
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
                .apply_custom_pred(
                    false,
                    "IsLog",
                    map!({"state_header" => state.state_header().array()}),
                    vec![Statement::None, st_action.clone()],
                )
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
            .apply_custom_pred(
                false,
                "IsWood",
                map!({"state_header" => state.state_header().array()}),
                vec![st_craft_wood.clone(), Statement::None],
            )
            .unwrap();
        tx2.set_guard(h, st_guard);
        tx2.end_action(scope_outer);

        eprintln!("{tx2}");
        let (st, tx2_out, stats) = tx2.finalize(&mut ctx).unwrap();
        print_stats(&stats);
        ctx.builder.reveal(&st).unwrap();
        solve_and_verify(ctx.builder);

        state.apply_tx(&tx2_out);

        // ---- Tx 3: CraftSticks ----

        let builder = MultiPodBuilder::new(&params, &vd_set);
        let mut ctx = BuildContext { builder, modules };

        let stick_a_initial = make_object(is_stick.clone(), &[]);
        let stick_b_initial = make_object(is_stick, &[]);

        let inputs = vec![wood.clone()];
        let witness = state.grounding_witness(&inputs);
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
                .apply_custom_pred(
                    false,
                    "IsWood",
                    map!({"state_header" => state.state_header().array()}),
                    vec![Statement::None, st_action.clone()],
                )
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
                vec![
                    None,
                    None,
                    Some(entry(&initials, "stick_a").expect("referenced container entry exists")),
                    None,
                    None,
                ],
                st_ins_a,
            ))
            .unwrap();
        let st_ins_b_anchored = ctx
            .builder
            .priv_op(Operation::replace_value_with_entry(
                vec![
                    None,
                    None,
                    Some(entry(&initials, "stick_b").expect("referenced container entry exists")),
                    None,
                    None,
                ],
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
            .apply_custom_pred(
                false,
                "IsStick",
                map!({"state_header" => state.state_header().array()}),
                vec![Statement::None, st_craft_sticks.clone(), Statement::None],
            )
            .unwrap();
        tx3.set_guard(h_a, st_is_stick_a);

        // stick_b: IsStick branch 3 = CraftSticks(other, obj, chain_start, chain_end)
        let st_is_stick_b = ctx
            .apply_custom_pred(
                false,
                "IsStick",
                map!({"state_header" => state.state_header().array()}),
                vec![Statement::None, Statement::None, st_craft_sticks.clone()],
            )
            .unwrap();
        tx3.set_guard(h_b, st_is_stick_b);

        tx3.end_action(scope_outer);

        eprintln!("{tx3}");
        let (st, tx3_out, stats) = tx3.finalize(&mut ctx).unwrap();
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

    #[test]
    fn test_grounds_three_inputs() {
        let events = Arc::new(crate::predicates::events_module());
        let txlib = Arc::new(crate::predicates::module());
        let craft = Arc::new(crate::predicates::crafting_test_module());
        let modules = vec![events, txlib.clone(), craft.clone()];

        let is_log =
            Value::from(Predicate::Custom(craft.predicate_ref_by_name("IsLog").unwrap()).hash());

        let mut state = TestState::empty(0);
        let params = Params::default();
        let vd_set = VDSet::new(&[]);

        // Spawn three logs (one FindLog tx each) and fold them into the live
        // set so the burn tx below can ground all three.
        let mut logs = Vec::new();
        for _ in 0..3 {
            let builder = MultiPodBuilder::new(&params, &vd_set);
            let mut ctx = BuildContext {
                builder,
                modules: modules.clone(),
            };
            let log_initial = make_object(is_log.clone(), &[]);
            let mut tx = TxBuilder::new(&mut ctx, &[], state.grounding_witness(&[]));
            let scope = tx.begin_action();
            let (log, st_insert, h) = tx.insert(&mut ctx, &log_initial);
            let st_find = ctx
                .apply_custom_pred_simple(false, "FindLog", vec![st_insert])
                .unwrap();
            let st_guard = ctx
                .apply_custom_pred(
                    false,
                    "IsLog",
                    map!({"state_header" => state.state_header().array()}),
                    vec![st_find, Statement::None],
                )
                .unwrap();
            tx.set_guard(h, st_guard);
            tx.end_action(scope);
            let (st, tx_out, _stats) = tx.finalize(&mut ctx).unwrap();
            ctx.builder.reveal(&st).unwrap();
            solve_and_verify(ctx.builder);
            state.apply_tx(&tx_out);
            logs.push(log);
        }

        // Burn all three logs in one tx: TxBuilder::new grounds three inputs,
        // exercising the three-input grounding leaf.
        let builder = MultiPodBuilder::new(&params, &vd_set);
        let mut ctx = BuildContext { builder, modules };

        let inputs = logs.clone();
        let witness = state.grounding_witness(&inputs);
        let mut burn = TxBuilder::new(&mut ctx, &inputs, witness);

        for log in &logs {
            let scope = burn.begin_action();
            let (st_del, h) = burn.delete(&mut ctx, log);
            let st_action = ctx
                .apply_custom_pred_simple(false, "DeleteLog", vec![st_del])
                .unwrap();
            let st_guard = ctx
                .apply_custom_pred(
                    false,
                    "IsLog",
                    map!({"state_header" => state.state_header().array()}),
                    vec![Statement::None, st_action],
                )
                .unwrap();
            burn.set_guard(h, st_guard);
            burn.end_action(scope);
        }

        eprintln!("{burn}");
        let (st, burn_out, stats) = burn.finalize(&mut ctx).unwrap();
        print_stats(&stats);
        ctx.builder.reveal(&st).unwrap();
        solve_and_verify(ctx.builder);

        for log in &logs {
            assert!(
                burn_out
                    .nullifiers
                    .contains(&Value::from(compute_nullifier(log)))
                    .unwrap()
            );
        }
    }
}
