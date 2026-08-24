//! Object states and the values derived from them.
//!
//! An object state is a pod2 dictionary containing a class-guard `type`,
//! a spending `key`, and a `stable_identifier` preserved by mutations.
//! This module provides the shared field accessors, keyed hashes, and
//! dictionary transforms.

use std::collections::HashMap;

use pod2::middleware::{
    EMPTY_VALUE, Hash, Statement, StrKey, Value, containers::Dictionary, hash_values,
};
use pod2utils::{dict, macros::BuildContext, op, rand_raw_value};

pub(crate) const OBJECT_NULLIFIER_VERSION: &str = "txlib-nullifier-v1";
pub(crate) const ENDORSEMENT_VERSION: &str = "txlib-endorsement-v1";

/// Commit to the exact `{state_header, tx_commitment}` transaction context.
///
/// `TxFinalized` exposes this value and each spend endorsement binds to it.
/// Verifiers reconstruct it from `state_root` and `tx_final`, rejecting
/// contexts with extra entries.
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

/// Prove authorization to spend `old` in `context`.
///
/// Returns the derived nullifier and `EndorseSpend` statement. `reveal`
/// controls whether the statement is public. `context` accepts either the
/// context dictionary value or its commitment.
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

/// Return `obj` with its key replaced by the `EMPTY_VALUE` sentinel.
///
/// This intermediate is a `Rekey` witness, never an event object. A prover
/// without the old key can reconstruct it from disclosed non-key fields and
/// validate those fields against the key-erasure statement's commitment.
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

/// Stamp `stable_identifier = commitment(initial)` into the dictionary.
/// This matches the relationship proved by `TxInsert`.
pub fn with_stable_identifier(initial: &Dictionary) -> Dictionary {
    let stable_identifier = Value::from(initial.commitment());
    let mut new = initial.clone();
    new.insert(&StrKey::from(STABLE_IDENTIFIER_FIELD), &stable_identifier)
        .unwrap();
    new
}
