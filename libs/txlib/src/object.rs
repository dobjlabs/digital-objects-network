//! Object states and the values derived from them.
//!
//! An object state is a pod2 dictionary carrying at least `type` (the
//! hash of its class guard), `key` (the secret that authorizes spending
//! it), and `stable_identifier` (its identity across mutations). This
//! module holds the field accessors, the nullifier derivation keyed on
//! `key`, and the small dict transforms the builder and its callers
//! share.

use std::collections::HashMap;

use pod2::middleware::{EMPTY_VALUE, Hash, StrKey, Value, containers::Dictionary, hash_values};
use pod2utils::rand_raw_value;

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
