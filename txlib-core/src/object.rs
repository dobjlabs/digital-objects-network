//! Object — a typed key/value record with a deterministic SHA-256 commitment.
//!
//! Replaces pod2's `Dictionary` for object state. Fields are stored in a
//! `BTreeMap` so iteration is sorted by key, which makes the commitment
//! deterministic without an explicit sort step.
//!
//! Two fields are required by convention:
//! - `key`       — random 32-byte secret used in nullifier derivation
//! - `blueprint` — class name (e.g. `"Wood"`)
//!
//! The commitment scheme:
//! ```text
//! commitment = SHA256( DOBJ-OBJ
//!                   || u32_le(field_count)
//!                   || for each (key, value) in sorted order:
//!                          u32_le(key_len) || key_bytes
//!                       || value.commitment()
//!                    )
//! ```
//! Hashing field commitments rather than raw value bytes lets the SMT use
//! `value.commitment()` directly without re-hashing the whole object.

use alloc::collections::BTreeMap;
use alloc::string::String;
use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

use crate::hash::{Hash, domain, sha256};
use crate::value::Value;

pub const FIELD_KEY: &str = "key";
pub const FIELD_BLUEPRINT: &str = "blueprint";

#[derive(
    Clone, Debug, PartialEq, Eq, Default, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub struct Object {
    pub fields: BTreeMap<String, Value>,
}

impl Object {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.fields.get(key)
    }

    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<Value>) -> Option<Value> {
        self.fields.insert(key.into(), value.into())
    }

    pub fn key(&self) -> Option<Hash> {
        match self.fields.get(FIELD_KEY) {
            Some(Value::Hash(h)) => Some(*h),
            _ => None,
        }
    }

    pub fn blueprint(&self) -> Option<&str> {
        match self.fields.get(FIELD_BLUEPRINT) {
            Some(Value::String(s)) => Some(s.as_str()),
            _ => None,
        }
    }

    /// Deterministic SHA-256 commitment over the (sorted) fields.
    pub fn commitment(&self) -> Hash {
        let mut buf = alloc::vec::Vec::with_capacity(8 + 4 + self.fields.len() * 64);
        buf.extend_from_slice(domain::OBJECT);
        buf.extend_from_slice(&(self.fields.len() as u32).to_le_bytes());
        for (k, v) in &self.fields {
            let kb = k.as_bytes();
            buf.extend_from_slice(&(kb.len() as u32).to_le_bytes());
            buf.extend_from_slice(kb);
            buf.extend_from_slice(v.commitment().as_bytes());
        }
        sha256(&buf)
    }
}

/// Convenience builder for constructing literal objects in tests / actions.
#[macro_export]
macro_rules! object {
    ( $( $key:literal => $value:expr ),* $(,)? ) => {{
        let mut o = $crate::object::Object::new();
        $( o.insert($key.to_string(), $crate::value::Value::from($value)); )*
        o
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commitment_is_deterministic_across_inserts() {
        let mut a = Object::new();
        a.insert("blueprint".to_string(), "Wood");
        a.insert("key".to_string(), Hash([1u8; 32]));

        let mut b = Object::new();
        b.insert("key".to_string(), Hash([1u8; 32]));
        b.insert("blueprint".to_string(), "Wood");

        // BTreeMap sorts → insert order doesn't matter.
        assert_eq!(a.commitment(), b.commitment());
    }

    #[test]
    fn commitment_changes_with_field_change() {
        let mut a = Object::new();
        a.insert("blueprint", "Wood");
        a.insert("key", Hash([1u8; 32]));
        let c1 = a.commitment();

        a.insert("key", Hash([2u8; 32]));
        let c2 = a.commitment();

        assert_ne!(c1, c2);
    }

    #[test]
    fn missing_key_returns_none() {
        let o = Object::new();
        assert!(o.key().is_none());
        assert!(o.blueprint().is_none());
    }

    #[test]
    fn typed_accessors() {
        let mut o = Object::new();
        let key = Hash([0xab; 32]);
        o.insert("key", key);
        o.insert("blueprint", "WoodPick");
        o.insert("durability", 100i64);
        assert_eq!(o.key(), Some(key));
        assert_eq!(o.blueprint(), Some("WoodPick"));
        assert_eq!(o.fields.get("durability"), Some(&Value::Int(100)));
    }

    #[test]
    fn borsh_roundtrip() {
        let mut o = Object::new();
        o.insert("key", Hash([7u8; 32]));
        o.insert("blueprint", "Stone");
        o.insert("durability", 50i64);

        let bytes = borsh::to_vec(&o).unwrap();
        let o2: Object = borsh::from_slice(&bytes).unwrap();
        assert_eq!(o, o2);
        // Borsh is canonical, so the commitment survives roundtrip.
        assert_eq!(o.commitment(), o2.commitment());
    }

    #[test]
    fn macro_builds_object() {
        let key = Hash([3u8; 32]);
        let o = object! {
            "blueprint" => "Wood",
            "key" => key,
            "durability" => 99i64,
        };
        assert_eq!(o.blueprint(), Some("Wood"));
        assert_eq!(o.key(), Some(key));
    }

    #[test]
    fn empty_object_has_stable_commitment() {
        let a = Object::new();
        let b = Object::new();
        assert_eq!(a.commitment(), b.commitment());
        // Any nonempty object is distinct.
        let mut c = Object::new();
        c.insert("k", "v");
        assert_ne!(a.commitment(), c.commitment());
    }
}
