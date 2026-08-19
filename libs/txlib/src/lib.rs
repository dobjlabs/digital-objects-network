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
//! The `replay` submodule contains the predicate-tree construction
//! invoked by `finalize`.
//!
//! # Module layout
//!
//! - `object` -- object states and the values derived from them
//!   (field accessors, nullifier and endorsement hashes, dict
//!   transforms). No dependency on the builder.
//! - `state_header` -- the committed state view a transaction grounds
//!   against, and the witness carrying its membership proofs.
//! - `contribute` -- what a party proves about objects it holds so
//!   another party can assemble a transaction over them.
//! - `replay` -- the finalize-time walk over the recorded event tree.
//! - [`predicates`] -- the podlang sources and their compiled modules.
//!
//! This module holds the rest: the recorded event tree, [`TxBuilder`]
//! and its `finalize`, and the two enums naming the sides of a
//! mutation ([`ObjSide`] and [`ConsumedSide`]), which are on every
//! mutate path rather than only the joint ones.

pub mod predicates;

mod contribute;
mod object;
mod replay;
mod state_header;

pub use contribute::{ObjectOpenings, SpendAuthorization, TransferAcceptance, TransferOffer};
pub use object::{
    STABLE_IDENTIFIER_FIELD, compute_nullifier, context_commitment, erased_key_state, new_obj,
    object_key_hash, object_nullifier_from_key_hash, object_nullifier_hash,
    object_stable_identifier, object_type, with_stable_identifier,
};
pub(crate) use object::{obj_with_key, prove_endorse_spend};
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

/// One side of a mutation: either a state this builder holds and can
/// open, or one a counterparty contributed openings for.
#[derive(Clone, Debug)]
pub enum ObjSide {
    Held(Dictionary),
    Contributed(Box<ObjectOpenings>),
}

impl ObjSide {
    pub fn commitment(&self) -> Hash {
        match self {
            Self::Held(d) => d.commitment(),
            Self::Contributed(o) => o.commitment,
        }
    }

    /// The value this state is stored as in the live set, and hashed as
    /// in the chain. A container's value is its commitment, so both
    /// variants agree.
    pub fn value(&self) -> Value {
        Value::from(self.commitment())
    }

    pub fn type_value(&self) -> Value {
        match self {
            Self::Held(d) => object_type(d),
            Self::Contributed(o) => o.type_value.clone(),
        }
    }

    pub fn stable_identifier(&self) -> Value {
        match self {
            Self::Held(d) => object_stable_identifier(d),
            Self::Contributed(o) => o.stable_identifier.clone(),
        }
    }

    /// The entry this side's `stable_identifier` is read from: its own
    /// dict when held, its holder's imported `DictContains` statement
    /// when contributed. Both forms are accepted directly as an
    /// `EqualFromEntries` argument, so the two-sided identity clause is
    /// one operation regardless of which parties hold which side.
    pub(crate) fn stable_identifier_entry(&self) -> OperationArg {
        match self {
            Self::Held(d) => OperationArg::from((d, STABLE_IDENTIFIER_FIELD)),
            Self::Contributed(o) => OperationArg::Statement(o.st_stable_identifier.clone()),
        }
    }

    /// `DictContains(self, "type", type_value)`, proven locally for a
    /// held state and reused from the contribution otherwise.
    pub(crate) fn type_opening(&self, ctx: &mut BuildContext, type_value: &Value) -> Statement {
        match self {
            Self::Held(d) => ctx
                .builder
                .priv_op(op!(DictContains(d, "type", type_value.clone())))
                .unwrap(),
            Self::Contributed(o) => o.st_type.clone(),
        }
    }

    pub(crate) fn dict(&self) -> Option<&Dictionary> {
        match self {
            Self::Held(d) => Some(d),
            Self::Contributed(_) => None,
        }
    }
}

/// Prove `TxMutate` for one recorded mutation, from whatever each side
/// supplies.
///
/// No clause here opens `new` or `old`: the type openings come from each
/// side (proven locally or contributed), the identity clause takes one
/// entry from each, and the two chain hashes work on commitments. That
/// is what lets either party assemble a mutation over an object the
/// other one holds.
///
/// `chain` is the position after this event, so a party proving this
/// outside a `TxBuilder` has to derive both positions from the agreed
/// event sequence.
pub(crate) fn prove_tx_mutate(
    ctx: &mut BuildContext,
    prev_chain: Hash,
    chain: Hash,
    old: &ObjSide,
    new: &ObjSide,
) -> Statement {
    let event_hash = hash_values(&[old.value(), new.value()]);
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

/// The consumed side of a mutation, which needs more than field
/// openings: spending a state requires its nullifier and its owner's
/// endorsement, and both derive from the object's `key`.
///
/// The two variants are the only two coherent combinations. `Held` means
/// this builder has the key and derives both itself at finalize time,
/// once the transaction context exists. `Contributed` means another
/// party holds the key and supplied both against a negotiated context.
#[derive(Clone, Debug)]
pub enum ConsumedSide {
    Held(Dictionary),
    /// Boxed: the contributed side carries two statements, and this enum
    /// is stored per event in the builder's event list.
    Contributed(Box<ContributedSpend>),
}

/// What the owner of a state contributes when another party assembles
/// the transaction that spends it: field openings for the assembler's
/// `TxMutate` clauses, plus the authorization only the owner can prove.
#[derive(Clone, Debug)]
pub struct ContributedSpend {
    pub openings: ObjectOpenings,
    pub auth: SpendAuthorization,
}

impl ConsumedSide {
    /// Field openings for this side, discarding the spend authorization.
    pub(crate) fn openings(&self) -> ObjSide {
        match self {
            Self::Held(d) => ObjSide::Held(d.clone()),
            Self::Contributed(c) => ObjSide::Contributed(Box::new(c.openings.clone())),
        }
    }

    /// The consumed state's nullifier: derived from the key when held,
    /// taken from the owner's authorization when contributed.
    pub(crate) fn nullifier(&self) -> Hash {
        match self {
            Self::Held(d) => compute_nullifier(d),
            Self::Contributed(c) => c.auth.nullifier,
        }
    }

    /// The owner's `EndorseSpend` statement, when another party proved
    /// it. `None` means replay builds it from the key at finalize time.
    pub(crate) fn endorsement(&self) -> Option<&Statement> {
        match self {
            Self::Held(_) => None,
            Self::Contributed(c) => Some(&c.auth.st_endorsement),
        }
    }

    pub(crate) fn held_dict(&self) -> Option<&Dictionary> {
        match self {
            Self::Held(d) => Some(d),
            Self::Contributed(_) => None,
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
        new: ObjSide,
        /// The consumed state, carrying whatever this builder knows about
        /// it: the dict when it holds the key, or its owner's openings
        /// and spend authorization when another party does.
        old: ConsumedSide,
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
            ChainEvent::Mutate { old, new, .. } => match (old.held_dict(), new.dict()) {
                (Some(o), Some(n)) => writeln!(f, "{pad}mutate {}", mutation_diff(o, n))?,
                // At least one side was contributed by another party, so
                // we hold only its commitment and cannot diff fields.
                _ => writeln!(
                    f,
                    "{pad}mutate {} -> {} (contributed)",
                    old.openings().commitment(),
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

    /// Create a transaction builder from input *commitments* rather than
    /// dicts, for assembling a transaction over objects held by other
    /// parties. Grounding never opens an input, so a commitment plus the
    /// witness entry keyed by it is all this needs.
    pub fn new_from_commitments(
        ctx: &mut BuildContext,
        inputs: &[Hash],
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

    /// Record a mutation of a state this builder holds. Emits TxMutate,
    /// updates live set and nullifiers. Must be called inside an open
    /// action scope. Returns the TxMutate statement and a handle for
    /// guard attachment.
    pub fn mutate(
        &mut self,
        ctx: &mut BuildContext,
        new: &Dictionary,
        old: &Dictionary,
    ) -> (Statement, EventHandle) {
        self.mutate_joint(
            ctx,
            &ObjSide::Held(new.clone()),
            &ConsumedSide::Held(old.clone()),
        )
    }

    /// Record a mutation where either side may be held by another party.
    ///
    /// The `TxMutate` statement is assembled here from whichever
    /// openings each side supplies, so the one clause that spans both
    /// sides needs nothing from a contributed side but the plain
    /// `DictContains` its holder proved: no round trip.
    pub fn mutate_joint(
        &mut self,
        ctx: &mut BuildContext,
        new: &ObjSide,
        old: &ConsumedSide,
    ) -> (Statement, EventHandle) {
        assert!(
            !self.action_stack.is_empty(),
            "mutate must be called inside an action scope",
        );
        let old_openings = old.openings();

        let prev = self.chain;
        let event_hash = hash_values(&[old_openings.value(), new.value()]);
        self.chain = hash_values(&[Value::from(prev), Value::from(event_hash)]);
        self.live.delete(&old_openings.value()).unwrap();
        self.live.insert(&new.value()).unwrap();
        self.nullifiers
            .insert(&Value::from(old.nullifier()))
            .unwrap();

        let new_type = new.type_value();
        assert_eq!(
            new_type,
            old_openings.type_value(),
            "mutate must preserve object type"
        );
        assert_eq!(
            new.stable_identifier(),
            old_openings.stable_identifier(),
            "mutate must preserve object stable identifier"
        );

        let st = prove_tx_mutate(ctx, prev, self.chain, &old_openings, new);
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

    /// Record a transfer of control: mutate `old` into an otherwise
    /// identical state under `new_key`, and prove the `Rekey` action
    /// predicate over it. Must be called inside an open action scope.
    ///
    /// Returns the new object state, the `Rekey` statement (to be placed
    /// in the class guard's Rekey branch by the caller), and the event
    /// handle for guard attachment.
    ///
    /// This is the single-party path: one builder holds both the old key
    /// and the new one. See [`TxBuilder::rekey_receive`] for the
    /// two-party split.
    pub fn rekey(
        &mut self,
        ctx: &mut BuildContext,
        old: &Dictionary,
        new_key: Value,
    ) -> (Dictionary, Statement, EventHandle) {
        let mid = erased_key_state(old);
        let (st_mutate, handle) = {
            let new = obj_with_key(&mid, new_key.clone());
            self.mutate(ctx, &new, old)
        };
        let st_erase = ctx
            .builder
            .priv_op(op!(DictUpdate(old, "key", EMPTY_VALUE, mid)))
            .unwrap();
        self.apply_rekey(ctx, &mid, new_key, st_erase, st_mutate, handle)
    }

    /// Record the receiving half of a two-party transfer: take control
    /// of a state held by another party by putting it under `new_key`.
    ///
    /// `offer` is the sender's whole contribution (see
    /// [`TransferOffer::prove`]), none of which reveals its key. `mid` is
    /// the erased-key state, which the receiver reconstructs from the
    /// sender's disclosed non-key fields via [`erased_key_state`].
    /// Reconstructing it is also the receiver's check on that
    /// disclosure: the commitment has to match the one in the sender's
    /// key-erasing statement, and `Rekey` will not prove otherwise.
    ///
    /// The two rekey paths cannot be collapsed into one: only the
    /// current owner can prove the key-erasing update, and only the
    /// receiver knows the new key, so which statements each party can
    /// produce differs by construction.
    pub fn rekey_receive(
        &mut self,
        ctx: &mut BuildContext,
        offer: &TransferOffer,
        mid: &Dictionary,
        new_key: Value,
    ) -> (Dictionary, Statement, EventHandle) {
        let (st_mutate, handle) = {
            let new = obj_with_key(mid, new_key.clone());
            self.mutate_joint(ctx, &ObjSide::Held(new), &offer.consumed_side())
        };
        self.apply_rekey(
            ctx,
            mid,
            new_key,
            offer.st_key_erasure.clone(),
            st_mutate,
            handle,
        )
    }

    /// Record the sending half of a two-party transfer: hand a state
    /// this builder holds to the party that proved `acceptance`.
    ///
    /// The mirror of [`TxBuilder::rekey_receive`], for when the sender
    /// assembles. The receiver had to prove the `Rekey` action itself
    /// (its new key is a private wildcard of that predicate), so this
    /// method records the event against the receiver's statements rather
    /// than building the action. The consumed side is held locally, so
    /// no [`SpendAuthorization`] is needed: replay derives the
    /// endorsement from `old`'s key at finalize time.
    ///
    /// Returns the event handle. Unlike the other recorders this yields
    /// no action statement to wrap in a guard: the receiver proved the
    /// action and its guard, so pass that guard to
    /// [`TxBuilder::set_guard`]. The new state stays with the receiver;
    /// this party only ever sees its commitment.
    pub fn rekey_send(
        &mut self,
        ctx: &mut BuildContext,
        old: &Dictionary,
        acceptance: &TransferAcceptance,
    ) -> EventHandle {
        let (_, handle) = self.mutate_joint(
            ctx,
            &acceptance.obj_side(),
            &ConsumedSide::Held(old.clone()),
        );
        record(&mut self.stats, "Rekey");
        handle
    }

    /// Shared tail of both rekey paths: set the new key on the
    /// erased-key state and apply `Rekey` over the two updates and the
    /// mutation. Keeps the predicate's clause order in one place.
    fn apply_rekey(
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

        // The per-transaction context: binds the grounding state header
        // and the final transaction commitment together. The live and
        // nullifier sets are maintained incrementally by the event
        // recorders, so after_tx (and therefore the context) is known
        // before any replay statement is built. The entries carry the
        // full container values (not just their hashes) so replay-time
        // anchored-key rebinds can open the dict against them.
        let context = dict!({
            "state_header" => self.state_header.array(),
            "tx_commitment" => after_tx.clone()
        });

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
        let (st_replay, _, _, _) = replay::Replayer::new(ctx, &mut stats, &context)
            .build_replay_actions(&self.events, self.chain_start, frame);

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
        // Surface the final nullifier and live sets as public args, and
        // pin the context's two entries: the inner state header (used to
        // ground the inputs) and tx_commitment == tx_final (which closes
        // the endorsement loop for every spend in the replay).
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

    /// Inputs are identified by commitment: grounding is
    /// `ArrayContains(created, index, commitment)` plus set insertions,
    /// none of which open the object. That is what lets an assembler
    /// ground an input held by another party.
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

/// Prove `ArrayContains(created, index, commitment)` for one input object
/// against the global created-set commitment `created_root`, passed as a plain
/// literal.
///
/// The created set stores object commitments at sequential indices, and a
/// container's value is its commitment, so this never needs to open the object.
/// That is what lets an assembler ground an input held by another party. The
/// index comes from the grounding witness.
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
