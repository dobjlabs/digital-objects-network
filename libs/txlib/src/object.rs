//! Object states and the values derived from them.
//!
//! An object state is a pod2 dictionary carrying at least `type` (the
//! hash of its class guard), `key` (the secret that authorizes spending
//! it), and `stable_identifier` (its identity across mutations). This
//! module holds the field accessors, the hash derivations keyed on
//! `key` (nullifier and spend endorsement), and the small dict
//! transforms the builder and its callers share.

use std::collections::HashMap;

use pod2::middleware::{
    EMPTY_VALUE, Hash, Statement, StrKey, Value, containers::Dictionary, hash_values,
};
use pod2utils::{dict, macros::BuildContext, op, rand_raw_value};

pub(crate) const OBJECT_NULLIFIER_VERSION: &str = "txlib-nullifier-v1";
pub(crate) const ENDORSEMENT_VERSION: &str = "txlib-endorsement-v1";

/// Commitment of the per-transaction context dict
/// `{state_header, tx_commitment}` for `(state_root, tx_final)`.
///
/// This is the value `TxFinalized` exposes as its first public arg and
/// the value every spend endorsement hashes. Verifiers rebuild it from
/// the published state root and tx_final; because the reconstruction
/// is an exact two-entry dict, a prover-supplied context padded with
/// extra keys can never match.
pub fn context_commitment(state_root: Hash, tx_final: Hash) -> Hash {
    dict!({
        "state_header" => state_root,
        "tx_commitment" => tx_final
    })
    .commitment()
}

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

/// Extract the `stable_identifier` field, stamped by `TxInsert` and
/// preserved by every `TxMutate`.
pub fn object_stable_identifier(obj: &Dictionary) -> Value {
    obj.get(&StrKey::from(STABLE_IDENTIFIER_FIELD))
        .expect("object dict lookup")
        .expect("object missing stable identifier (must come from TxBuilder::insert)")
}

/// Prove `EndorseSpend(context, nullifier, old)`: the object owner's
/// complete authorization to spend `old` in the transaction committing
/// to `context`. Returns the nullifier alongside the statement.
///
/// Every clause but the version tags opens `old` at `key`, so only the
/// owner can prove this. `reveal` makes the statement public, which a
/// party contributing to someone else's assembly needs and replay's own
/// local spends do not. `context` is taken as a `Value` so each caller
/// can pass whichever form it holds: a container's value is its
/// commitment, so the dict and the bare hash agree.
pub fn prove_endorse_spend(
    ctx: &mut BuildContext,
    reveal: bool,
    context: Value,
    old: &Dictionary,
) -> (Hash, Statement) {
    let okh = object_key_hash(old).expect("object missing required key field");
    let nullifier = object_nullifier_from_key_hash(okh);
    let (tagged, endorsement) = endorsement_hashes(&context, old);

    let op_h1 = ctx
        .builder
        .priv_op(op!(Hash(old, (old, "key"), okh)))
        .unwrap();
    let op_h2 = ctx
        .builder
        .priv_op(op!(Hash(okh, OBJECT_NULLIFIER_VERSION, nullifier)))
        .unwrap();
    let op_e1 = ctx
        .builder
        .priv_op(op!(Hash(context, ENDORSEMENT_VERSION, tagged)))
        .unwrap();
    let op_e2 = ctx
        .builder
        .priv_op(op!(Hash((old, "key"), tagged, endorsement)))
        .unwrap();
    let st = ctx
        .apply_custom_pred_simple(reveal, "EndorseSpend", vec![op_h1, op_h2, op_e1, op_e2])
        .unwrap();
    (nullifier, st)
}

/// The spend-endorsement hash pair for `(context, obj)`:
/// `tagged = H(context, "txlib-endorsement-v1")`,
/// `endorsement = H(obj.key, tagged)`. Computing the second hash needs
/// the object's `key` entry, so only the object's owner can endorse a
/// spend for a given transaction context.
pub(crate) fn endorsement_hashes(context: &Value, obj: &Dictionary) -> (Hash, Hash) {
    let key = obj
        .get(&StrKey::from("key"))
        .expect("object dict lookup")
        .expect("object missing required key field");
    let tagged = hash_values(&[context.clone(), Value::from(ENDORSEMENT_VERSION)]);
    let endorsement = hash_values(&[key, Value::from(tagged)]);
    (tagged, endorsement)
}

/// Return a clone of `obj` with its `key` field replaced.
pub fn obj_with_key(obj: &Dictionary, key: Value) -> Dictionary {
    let mut result = obj.clone();
    result.update(&StrKey::from("key"), &key).unwrap();
    result
}

/// The intermediate state a `Rekey` passes through: `obj` with its key
/// erased to the sentinel (`EMPTY_VALUE`, written `{}` in podlang).
///
/// This state is a witness inside the `Rekey` predicate and never
/// becomes an event object, so its (publicly computable) nullifier is
/// never emitted. It is public API because the receiving party in a
/// two-party transfer has to reconstruct it: they can do so from the
/// object's disclosed non-key fields alone, which is what lets them
/// prove their half of the transfer without learning the sender's key.
/// Checking the reconstruction against the commitment in the sender's
/// key-erasing statement is also what proves the sender disclosed
/// every non-key field honestly.
pub fn erased_key_state(obj: &Dictionary) -> Dictionary {
    obj_with_key(obj, Value::from(EMPTY_VALUE))
}

pub fn new_obj() -> Dictionary {
    let mut map = HashMap::new();
    map.insert(StrKey::from("key"), Value::from(rand_raw_value()));
    map.insert(StrKey::from("work"), Value::from(EMPTY_VALUE));
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
