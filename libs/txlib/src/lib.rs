//! Transaction predicates for verifiable state transitions.
//!
//! A transaction consumes grounded input objects, records
//! insert/mutate/delete events in a hash chain, and produces a
//! `TxFinalized` proof. Replay verifies the chain and exposes the
//! transaction context, final commitment, nullifiers, and live set.
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
//! - `object` -- object states, field accessors, hashes, and dictionary
//!   transforms.
//! - `state_header` -- the committed state view a transaction grounds
//!   against, and the witness carrying its membership proofs.
//! - `replay` -- the finalize-time walk over the recorded event tree.
//! - [`predicates`] -- the podlang sources and their compiled modules.
//!
//! This module contains [`TxBuilder`], its event tree, and two fact
//! bundles. [`StateFacts`] supplies the commitment, `type`, and
//! `stable_identifier` needed by a mutation. [`SpendFacts`] adds the
//! nullifier and endorsement needed to consume a state. Both accept a
//! local dictionary or statements proven elsewhere; they describe how
//! facts are supplied, not who owns the state.

pub mod predicates;

mod object;
mod replay;
mod state_header;
pub mod test_support;

pub use object::{
    STABLE_IDENTIFIER_FIELD, compute_nullifier, context_commitment, erased_key_state, new_obj,
    obj_with_key, object_key_hash, object_nullifier_from_key_hash, object_nullifier_hash,
    object_stable_identifier, object_type, prove_endorse_spend, with_stable_identifier,
};
pub use state_header::{
    GroundingWitness, RECORD_STATE_HEADER_FIELDS, RECORD_STATE_HEADER_PODLANG,
    STATE_HEADER_BLOCK_HASH_SLOT, STATE_HEADER_BLOCK_NUMBER_SLOT,
    STATE_HEADER_BLOCK_TIMESTAMP_SLOT, STATE_HEADER_CREATED_SLOT, STATE_HEADER_NULLIFIERS_SLOT,
    STATE_HEADER_PRIOR_STATE_HISTORY_SLOT, StateHeader,
};

use std::sync::Arc;

use pod2::{
    frontend::{Operation, OperationArg},
    middleware::{
        EMPTY_VALUE, Hash, NativeOperation, OperationAux, OperationType, Statement, StrKey, Value,
        containers::{Dictionary, Set},
        hash_values,
    },
};
use pod2utils::{dict, macros::BuildContext, map, op, set, st_custom};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

// ============================================================================
// Transaction output and mutation sides
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

/// Openings needed to act on a state without its dictionary.
/// Callers prove the statements before passing them to the builder.
#[derive(Clone, Debug)]
pub struct StateOpenings {
    pub commitment: Hash,
    pub type_value: Value,
    pub stable_identifier: Value,
    /// `DictContains(obj, "type", type_value)`
    pub st_type: Statement,
    /// `DictContains(obj, "stable_identifier", stable_identifier)`
    pub st_stable_identifier: Statement,
}

/// State facts supplied by a dictionary or by previously proven openings.
#[derive(Clone, Debug)]
pub enum StateFacts {
    Dict(Dictionary),
    Statements(Box<StateOpenings>),
}

impl StateFacts {
    pub fn commitment(&self) -> Hash {
        match self {
            Self::Dict(d) => d.commitment(),
            Self::Statements(o) => o.commitment,
        }
    }

    pub fn value(&self) -> Value {
        Value::from(self.commitment())
    }

    pub fn type_value(&self) -> Value {
        match self {
            Self::Dict(d) => object_type(d),
            Self::Statements(o) => o.type_value.clone(),
        }
    }

    pub fn stable_identifier(&self) -> Value {
        match self {
            Self::Dict(d) => object_stable_identifier(d),
            Self::Statements(o) => o.stable_identifier.clone(),
        }
    }

    pub(crate) fn stable_identifier_entry(&self) -> OperationArg {
        match self {
            Self::Dict(d) => OperationArg::from((d, STABLE_IDENTIFIER_FIELD)),
            Self::Statements(o) => OperationArg::Statement(o.st_stable_identifier.clone()),
        }
    }

    /// `DictContains(self, "type", type_value)`, proven here for a held
    /// state and reused from the openings otherwise.
    pub(crate) fn type_opening(&self, ctx: &mut BuildContext, type_value: &Value) -> Statement {
        match self {
            Self::Dict(d) => ctx
                .builder
                .priv_op(op!(DictContains(d, "type", type_value.clone())))
                .unwrap(),
            Self::Statements(o) => o.st_type.clone(),
        }
    }

    pub(crate) fn dict(&self) -> Option<&Dictionary> {
        match self {
            Self::Dict(d) => Some(d),
            Self::Statements(_) => None,
        }
    }
}

/// Prove `TxMutate` from dictionaries, previously proven openings, or both.
///
/// The proof opens neither state dictionary. It uses supplied openings for
/// `type` and `stable_identifier`, and commitments for the chain hashes.
/// `prev_chain` and `chain` are the positions before and after the event.
/// This function only produces a statement; [`TxBuilder`] must still record
/// the event and attach guard evidence.
pub fn prove_tx_mutate(
    ctx: &mut BuildContext,
    prev_chain: Hash,
    chain: Hash,
    old: &StateFacts,
    new: &StateFacts,
) -> Statement {
    let event_hash = event_hash_mutate(old.value(), new.value());
    let type_value = new.type_value();
    let st_dc_new = new.type_opening(ctx, &type_value);
    let st_dc_old = old.type_opening(ctx, &type_value);
    let st_eq_stable_identifier = ctx
        .builder
        .priv_op(Operation::eq(
            old.stable_identifier_entry(),
            new.stable_identifier_entry(),
        ))
        .unwrap();
    let st_h1 = ctx
        .builder
        .priv_op(op!(Hash(old.value(), new.value(), event_hash)))
        .unwrap();
    let st_h2 = ctx
        .builder
        .priv_op(op!(Hash(prev_chain, event_hash, chain)))
        .unwrap();
    ctx.apply_custom_pred(
        false,
        "TxMutate",
        map!({"prev_chain" => prev_chain, "chain" => chain, "old" => old.value(), "new" => new.value(), "type" => type_value}),
        vec![st_dc_new, st_dc_old, st_eq_stable_identifier, st_h1, st_h2],
    )
    .unwrap()
}

/// [`StateFacts`] plus the nullifier and endorsement needed to consume it.
///
/// `Dict` derives both from the key during finalization. `Statements`
/// supplies both as proofs bound to a predetermined transaction context.
/// Keeping these cases distinct prevents an incomplete authorization.
#[derive(Clone, Debug)]
pub enum SpendFacts {
    Dict(Dictionary),
    /// Boxed to keep this enum small: it is stored per event.
    Statements(Box<SpendStatements>),
}

/// Openings and authorization supplied for a state whose key is unavailable.
#[derive(Clone, Debug)]
pub struct SpendStatements {
    pub openings: StateOpenings,
    pub nullifier: Hash,
    /// `EndorseSpend(context, nullifier, old)`
    pub endorsement: Statement,
}

impl SpendFacts {
    /// Project down to the identity facts, dropping the authorization.
    pub(crate) fn state(&self) -> StateFacts {
        match self {
            Self::Dict(d) => StateFacts::Dict(d.clone()),
            Self::Statements(c) => StateFacts::Statements(Box::new(c.openings.clone())),
        }
    }

    /// The consumed state's nullifier: derived from the key when held,
    /// taken from the supplied authorization otherwise.
    pub(crate) fn nullifier(&self) -> Hash {
        match self {
            Self::Dict(d) => compute_nullifier(d),
            Self::Statements(c) => c.nullifier,
        }
    }

    /// The `EndorseSpend` statement, when it was proven elsewhere.
    /// `None` means replay builds it from the key at finalize.
    pub(crate) fn endorsement(&self) -> Option<&Statement> {
        match self {
            Self::Dict(_) => None,
            Self::Statements(c) => Some(&c.endorsement),
        }
    }

    pub(crate) fn dict(&self) -> Option<&Dictionary> {
        match self {
            Self::Dict(d) => Some(d),
            Self::Statements(_) => None,
        }
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
        new: StateFacts,
        /// The consumed state, carrying whatever this builder knows about
        /// it: the dict when it holds the key, otherwise the openings and
        /// spend authorization proven by whoever does.
        old: SpendFacts,
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
// Chain arithmetic and tx-dict helpers
// ============================================================================

/// Chain seed of a transaction over `inputs`: `H(inputs, {})`.
pub fn chain_seed(inputs: &Set) -> Hash {
    hash_values(&[Value::from(inputs.commitment()), Value::from(EMPTY_VALUE)])
}

/// Event hash of an insert: `H({}, new)`.
pub fn event_hash_insert(new: Value) -> Hash {
    hash_values(&[Value::from(EMPTY_VALUE), new])
}

/// Event hash of a mutate: `H(old, new)`.
pub fn event_hash_mutate(old: Value, new: Value) -> Hash {
    hash_values(&[old, new])
}

/// Event hash of a delete: `H(old, {})`.
pub fn event_hash_delete(old: Value) -> Hash {
    hash_values(&[old, Value::from(EMPTY_VALUE)])
}

/// One chain step: `H(prev, event_hash)`.
pub fn chain_step(prev: Hash, event_hash: Hash) -> Hash {
    hash_values(&[Value::from(prev), Value::from(event_hash)])
}

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

/// A top-level replay scope dict: live and nullifiers with both chain
/// bounds zeroed. `TxFinalized` pins the zeroed bounds and
/// `ReplayAction` restores real ones per scope; the after set's
/// commitment is tx_final, the value the relayer publishes.
pub fn top_level_tx(live: &Set, nullifiers: &Set) -> Dictionary {
    let zero: Hash = EMPTY_VALUE.into();
    build_tx(live, nullifiers, zero, zero)
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
            ChainEvent::Mutate { old, new, .. } => match (old.dict(), new.dict()) {
                (Some(o), Some(n)) => writeln!(f, "{pad}mutate {}", mutation_diff(o, n))?,
                // At least one side is known only indirectly, so only
                // its commitment is available and fields cannot be
                // diffed.
                _ => writeln!(
                    f,
                    "{pad}mutate {} -> {} (indirect)",
                    old.state().commitment(),
                    new.commitment()
                )?,
            },
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
        let commitments: Vec<Hash> = inputs.iter().map(|d| d.commitment()).collect();
        Self::new_from_commitments(ctx, &commitments, grounding)
    }

    /// Create a builder from input commitments and their grounding witness.
    /// Grounding does not open the input dictionaries.
    pub fn new_from_commitments(
        ctx: &mut BuildContext,
        inputs: &[Hash],
        grounding: Arc<GroundingWitness>,
    ) -> Self {
        let (st_inputs_grounded, inputs_set, stats) =
            Self::build_inputs_grounded(ctx, inputs, &grounding);
        let chain_start = chain_seed(&inputs_set);
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
        let event_hash = event_hash_insert(Value::from(new.clone()));
        self.chain = chain_step(prev, event_hash);
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

    /// Record a mutation when both state dictionaries are available.
    /// Returns the `TxMutate` statement and a handle for guard attachment.
    pub fn mutate_dicts(
        &mut self,
        ctx: &mut BuildContext,
        new: &Dictionary,
        old: &Dictionary,
    ) -> (Statement, EventHandle) {
        self.mutate(
            ctx,
            &StateFacts::Dict(new.clone()),
            &SpendFacts::Dict(old.clone()),
        )
    }

    /// Record a mutation from dictionaries, previously proven statements,
    /// or both. Returns the `TxMutate` statement and guard handle.
    pub fn mutate(
        &mut self,
        ctx: &mut BuildContext,
        new: &StateFacts,
        old: &SpendFacts,
    ) -> (Statement, EventHandle) {
        assert!(
            !self.action_stack.is_empty(),
            "mutate must be called inside an action scope",
        );
        let old_state = old.state();

        let prev = self.chain;
        let event_hash = event_hash_mutate(old_state.value(), new.value());
        self.chain = chain_step(prev, event_hash);
        self.live.delete(&old_state.value()).unwrap();
        self.live.insert(&new.value()).unwrap();
        self.nullifiers
            .insert(&Value::from(old.nullifier()))
            .unwrap();

        let new_type = new.type_value();
        assert_eq!(
            new_type,
            old_state.type_value(),
            "mutate must preserve object type"
        );
        assert_eq!(
            new.stable_identifier(),
            old_state.stable_identifier(),
            "mutate must preserve object stable identifier"
        );

        let st = prove_tx_mutate(ctx, prev, self.chain, &old_state, new);
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

    /// Rekey a locally available state and prove the `Rekey` action.
    ///
    /// Returns the new state, the action statement for the class guard,
    /// and the event handle. The state is otherwise unchanged.
    pub fn rekey(
        &mut self,
        ctx: &mut BuildContext,
        old: &Dictionary,
        new_key: Value,
    ) -> (Dictionary, Statement, EventHandle) {
        let mid = erased_key_state(old);
        let (st_mutate, handle) = {
            let new = obj_with_key(&mid, new_key.clone());
            self.mutate_dicts(ctx, &new, old)
        };
        let st_erase = ctx
            .builder
            .priv_op(op!(DictUpdate(old, "key", EMPTY_VALUE, mid)))
            .unwrap();
        self.prove_rekey(ctx, &mid, new_key, st_erase, st_mutate, handle)
    }

    /// Rekey a state using a key-erasure proof supplied by its holder.
    ///
    /// `mid` is the erased-key state reconstructed from disclosed non-key
    /// fields. Its commitment must match `st_key_erasure`. This builder
    /// can therefore prove `Rekey` without opening the consumed state.
    /// Key erasure and selection of `new_key` remain separate because each
    /// requires a value unavailable to the other prover.
    pub fn rekey_apply(
        &mut self,
        ctx: &mut BuildContext,
        consumed: &SpendFacts,
        st_key_erasure: Statement,
        mid: &Dictionary,
        new_key: Value,
    ) -> (Dictionary, Statement, EventHandle) {
        let (st_mutate, handle) = {
            let new = obj_with_key(mid, new_key.clone());
            self.mutate(ctx, &StateFacts::Dict(new), consumed)
        };
        self.prove_rekey(ctx, mid, new_key, st_key_erasure, st_mutate, handle)
    }

    /// Record a rekey already proven by the receiver of the new key.
    ///
    /// `old` is available locally; `new` is supplied through statements.
    /// Replay derives the spend authorization from `old`. This returns
    /// only an event handle because the caller already has guard evidence.
    pub fn rekey_record(
        &mut self,
        ctx: &mut BuildContext,
        old: &Dictionary,
        new: &StateFacts,
    ) -> EventHandle {
        let (_, handle) = self.mutate(ctx, new, &SpendFacts::Dict(old.clone()));
        record(&mut self.stats, "Rekey");
        handle
    }

    /// Set the new key and prove `Rekey` over both updates and the mutation.
    fn prove_rekey(
        &mut self,
        ctx: &mut BuildContext,
        mid: &Dictionary,
        new_key: Value,
        st_erase: Statement,
        st_mutate: Statement,
        handle: EventHandle,
    ) -> (Dictionary, Statement, EventHandle) {
        let new = obj_with_key(mid, new_key.clone());
        let st_set = ctx
            .builder
            .priv_op(op!(DictUpdate(mid, "key", new_key, new)))
            .unwrap();
        let st = ctx
            .apply_custom_pred_simple(false, "Rekey", vec![st_erase, st_set, st_mutate])
            .unwrap();
        record(&mut self.stats, "Rekey");
        (new, st, handle)
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
        let event_hash = event_hash_delete(Value::from(old.clone()));
        self.chain = chain_step(prev, event_hash);
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

        let before_tx = top_level_tx(&self.inputs_set, &set!());
        let after_tx = top_level_tx(&self.live, &self.nullifiers);

        // Bind every spend to both the grounding header and final transaction.
        // Full container values let replay open these entries through
        // anchored-key rebinds.
        let context = dict!({
            "state_header" => self.state_header.array(),
            "tx_commitment" => after_tx.clone()
        });

        // Top-level events are always actions, so skip ReplayContents.
        let empty_nullifiers = set!();
        let frame = replay::ReplayFrame {
            live: &self.inputs_set,
            nullifiers: &empty_nullifiers,
            chain_start: zero,
            chain_end: zero,
        };
        let (st_replay, _, _, _) = replay::Replayer::new(ctx, &mut stats, &context)
            .build_replay_actions(&self.events, self.chain_start, frame);

        // Rebind InputsGrounded to `before_tx.live` and
        // `state_header.created`. Separate calls are required for the dict key
        // and array index anchor types.
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
        // Pin every `before_tx` field. Otherwise arbitrary chain bounds could
        // pass through ReplayActions into tx_final.
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
        // Expose the final nullifiers and live set, then bind the context to
        // the grounding header and tx_final.
        let st_dc_ctx_header = ctx
            .builder
            .priv_op(op!(DictContains(
                context,
                "state_header",
                self.state_header.array()
            )))
            .unwrap();
        let st_dc_ctx_txfinal = ctx
            .builder
            .priv_op(op!(DictContains(context, "tx_commitment", after_tx)))
            .unwrap();
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
                vec![
                    st_dc_ctx_header,
                    st_dc_ctx_txfinal,
                    st_dc_null_after,
                    st_dc_live_after,
                ],
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
        inputs: &[Hash],
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

        let extend_set = |set: &Set, obj: &Hash| -> Set {
            let mut new_set = set.clone();
            new_set.insert(&Value::from(*obj)).unwrap();
            new_set
        };

        let prove_input = |ctx: &mut BuildContext, obj: &Hash| {
            prove_obj_in_created(ctx, created_root, grounding, *obj)
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

/// Prove that an input commitment occurs in the global created set.
/// The grounding witness supplies the index and Merkle proof; the object
/// dictionary remains unopened.
fn prove_obj_in_created(
    ctx: &mut BuildContext,
    created_root: Hash,
    grounding: &GroundingWitness,
    obj: Hash,
) -> Statement {
    let (index, proof) = grounding
        .created_proofs
        .get(&obj)
        .cloned()
        .expect("missing created-set proof in grounding witness");
    ctx.builder
        .priv_op(Operation(
            OperationType::Native(NativeOperation::ArrayContainsFromEntries),
            vec![
                Value::from(created_root).into(),
                Value::from(index).into(),
                Value::from(obj).into(),
            ],
            OperationAux::MerkleProof(proof),
        ))
        .unwrap()
}

#[cfg(test)]
mod tests;
