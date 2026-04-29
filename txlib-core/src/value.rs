//! Typed [`Value`] for [`Object`](crate::object::Object) fields.
//!
//! A `Value` is one of: signed integer, byte string, UTF-8 string, or [`Hash`].
//! This is the full type set needed by the craft-basics actions (`durability`
//! is `Int`, `key` is `Hash`, `blueprint` is `String`, `work` is `Hash`).
//!
//! Each variant has its own domain-separated commitment so two distinct
//! variants never collide. The serde representation uses serde's default
//! enum tagging — verbose but unambiguous, no special-case dispatch needed.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

use crate::hash::{Hash, domain, sha256_concat};

#[derive(
    Clone, Debug, PartialEq, Eq, Hash, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub enum Value {
    /// Signed 64-bit integer (e.g. `durability`).
    Int(i64),
    /// Raw byte string.
    Bytes(Vec<u8>),
    /// UTF-8 string (e.g. `blueprint`).
    String(String),
    /// Nested 32-byte hash (e.g. `key`, `work`).
    Hash(Hash),
}

impl Value {
    /// Domain-separated commitment of this value. Distinct variants never
    /// collide because each starts with a different 8-byte tag.
    pub fn commitment(&self) -> Hash {
        match self {
            Value::Int(i) => sha256_concat(&[domain::VALUE_INT, &i.to_le_bytes()]),
            Value::Bytes(b) => sha256_concat(&[
                domain::VALUE_BYTES,
                &(b.len() as u32).to_le_bytes(),
                b.as_slice(),
            ]),
            Value::String(s) => {
                let bytes = s.as_bytes();
                sha256_concat(&[
                    domain::VALUE_STR,
                    &(bytes.len() as u32).to_le_bytes(),
                    bytes,
                ])
            }
            Value::Hash(h) => sha256_concat(&[domain::VALUE_HASH, h.as_bytes()]),
        }
    }
}

impl From<i64> for Value {
    fn from(v: i64) -> Self {
        Value::Int(v)
    }
}
impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Value::String(v.to_string())
    }
}
impl From<String> for Value {
    fn from(v: String) -> Self {
        Value::String(v)
    }
}
impl From<Hash> for Value {
    fn from(v: Hash) -> Self {
        Value::Hash(v)
    }
}
impl From<Vec<u8>> for Value {
    fn from(v: Vec<u8>) -> Self {
        Value::Bytes(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinct_variants_never_collide() {
        // Same integer payload (1) across variants → distinct commitments.
        let int = Value::Int(1);
        let bytes = Value::Bytes(alloc::vec![1u8]);
        let s = Value::String("1".into());
        let h = Value::Hash(Hash([1u8; 32]));
        let cs = [
            int.commitment(),
            bytes.commitment(),
            s.commitment(),
            h.commitment(),
        ];
        for i in 0..cs.len() {
            for j in (i + 1)..cs.len() {
                assert_ne!(cs[i], cs[j], "variants {i} and {j} collided");
            }
        }
    }

    #[test]
    fn borsh_roundtrip() {
        for v in [
            Value::Int(-7),
            Value::Bytes(alloc::vec![0, 1, 2]),
            Value::String("blueprint".into()),
            Value::Hash(Hash([3u8; 32])),
        ] {
            let bytes = borsh::to_vec(&v).unwrap();
            let v2: Value = borsh::from_slice(&bytes).unwrap();
            assert_eq!(v, v2);
        }
    }

    #[test]
    fn serde_json_roundtrip() {
        let v = Value::Int(42);
        let s = serde_json::to_string(&v).unwrap();
        let v2: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v, v2);
    }

    #[test]
    fn commitment_is_deterministic() {
        let v1 = Value::String("hello".into());
        let v2 = Value::String("hello".into());
        assert_eq!(v1.commitment(), v2.commitment());
    }
}
