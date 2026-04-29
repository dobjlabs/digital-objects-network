//! Transaction and state-root commitments + nullifier derivation.
//!
//! ## Tx
//! A `Tx` is the public-facing record of one action invocation. Its
//! `tx_final` commitment is what gets stored in the synchronizer's
//! `transactions` set.
//!
//! ```text
//! tx_final = SHA256( DOBJ-TXF
//!                 || u32_le(action_id)
//!                 || live_root
//!                 || nullifiers_root
//!                 || action_nonce )
//! ```
//!
//! Where `live_root` and `nullifiers_root` are the roots of sparse Merkle trees
//! over the live commitments and nullifier hashes respectively, and
//! `action_nonce` is whatever bytes the action chose to make its tx unique
//! (typically derived from the new objects' random `key` fields). The
//! synchronizer rejects duplicate `tx_final`, so collisions are a hard fail.
//!
//! ## StateRoot
//! ```text
//! state_root = SHA256( DOBJ-STR
//!                   || i64_le(block_number)
//!                   || transactions_root
//!                   || nullifiers_root
//!                   || gsrs_root )
//! ```
//!
//! ## Nullifier
//! Same recipe as the pod2-era txlib, just with SHA-256 instead of Poseidon:
//! ```text
//! key_hash    = SHA256( DOBJ-NUK || obj_commitment || obj.key )
//! nullifier   = SHA256( DOBJ-NUL || key_hash || NULLIFIER_VERSION )
//! ```

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

use crate::hash::{Hash, domain, sha256_concat};
use crate::object::Object;

pub const NULLIFIER_VERSION: &[u8] = b"txlib-nullifier-v1";

#[derive(
    Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
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

    pub fn hash(&self) -> Hash {
        sha256_concat(&[
            domain::STATE_ROOT,
            &self.block_number.to_le_bytes(),
            self.transactions_root.as_bytes(),
            self.nullifiers_root.as_bytes(),
            self.gsrs_root.as_bytes(),
        ])
    }
}

/// Public-facing summary of one action invocation.
///
/// `live_root` and `nullifiers_root` are SMT roots; the underlying sets are
/// reconstructible by the synchronizer from `live_commitments` and
/// `nullifiers` once they're committed via `tx_final`.
#[derive(
    Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub struct Tx {
    /// Identifies which guest action produced this tx (dispatcher key).
    pub action_id: u32,
    /// SMT root over the new objects' commitments.
    pub live_root: Hash,
    /// SMT root over the consumed objects' nullifiers.
    pub nullifiers_root: Hash,
    /// Per-invocation nonce — typically `SHA256(action_id || sorted(new_obj_commitments))`,
    /// which forces uniqueness because every new object carries a fresh random `key`.
    pub action_nonce: Hash,
}

impl Tx {
    pub fn tx_final(&self) -> Hash {
        sha256_concat(&[
            domain::TX_FINAL,
            &self.action_id.to_le_bytes(),
            self.live_root.as_bytes(),
            self.nullifiers_root.as_bytes(),
            self.action_nonce.as_bytes(),
        ])
    }
}

/// `SHA256(DOBJ-NUK || obj_commitment || obj.key)`. Returns `None` if `obj`
/// is missing a `key` field of variant `Hash`.
pub fn object_key_hash(obj: &Object) -> Option<Hash> {
    let key = obj.key()?;
    let commitment = obj.commitment();
    Some(sha256_concat(&[
        domain::NULLIFIER_KEY,
        commitment.as_bytes(),
        key.as_bytes(),
    ]))
}

/// `SHA256(DOBJ-NUL || key_hash || NULLIFIER_VERSION)`.
pub fn nullifier_from_key_hash(key_hash: Hash) -> Hash {
    sha256_concat(&[domain::NULLIFIER, key_hash.as_bytes(), NULLIFIER_VERSION])
}

/// Full nullifier derivation. Panics if `obj` is missing its `key` field —
/// the guest treats a missing key as an unrecoverable input error.
pub fn compute_nullifier(obj: &Object) -> Hash {
    let kh = object_key_hash(obj).expect("object missing required `key: Hash` field");
    nullifier_from_key_hash(kh)
}

/// Convenience: derive a deterministic per-invocation nonce from the new
/// objects' commitments. Uniqueness comes from each object's random `key`.
pub fn action_nonce(action_id: u32, new_obj_commitments: &[Hash]) -> Hash {
    let mut buf = alloc::vec::Vec::with_capacity(4 + new_obj_commitments.len() * 32);
    buf.extend_from_slice(&action_id.to_le_bytes());
    // Caller is expected to pass commitments in canonical (sorted) order.
    for c in new_obj_commitments {
        buf.extend_from_slice(c.as_bytes());
    }
    sha256_concat(&[b"DOBJ-ANC", &buf])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Value;

    fn obj_with_key(key: u8) -> Object {
        let mut o = Object::new();
        o.insert("blueprint", "Wood");
        o.insert("key", Hash([key; 32]));
        o
    }

    #[test]
    fn state_root_hash_changes_with_each_field() {
        let base = StateRoot::new(1, Hash([1; 32]), Hash([2; 32]), Hash([3; 32]));
        let h = base.hash();

        let cases = [
            StateRoot::new(2, Hash([1; 32]), Hash([2; 32]), Hash([3; 32])),
            StateRoot::new(1, Hash([9; 32]), Hash([2; 32]), Hash([3; 32])),
            StateRoot::new(1, Hash([1; 32]), Hash([9; 32]), Hash([3; 32])),
            StateRoot::new(1, Hash([1; 32]), Hash([2; 32]), Hash([9; 32])),
        ];
        for c in &cases {
            assert_ne!(c.hash(), h);
        }
    }

    #[test]
    fn nullifier_depends_on_key() {
        let n1 = compute_nullifier(&obj_with_key(1));
        let n2 = compute_nullifier(&obj_with_key(2));
        assert_ne!(n1, n2);
    }

    #[test]
    fn nullifier_depends_on_other_fields() {
        let mut o1 = obj_with_key(1);
        let mut o2 = obj_with_key(1);
        o1.insert("durability", 100i64);
        o2.insert("durability", 99i64);
        // Different objects (different commitment) → different nullifiers.
        assert_ne!(compute_nullifier(&o1), compute_nullifier(&o2));
    }

    #[test]
    fn tx_final_changes_with_each_field() {
        let base = Tx {
            action_id: 1,
            live_root: Hash([1; 32]),
            nullifiers_root: Hash([2; 32]),
            action_nonce: Hash([3; 32]),
        };
        let h = base.tx_final();
        for variant in [
            Tx {
                action_id: 2,
                ..base.clone()
            },
            Tx {
                live_root: Hash([9; 32]),
                ..base.clone()
            },
            Tx {
                nullifiers_root: Hash([9; 32]),
                ..base.clone()
            },
            Tx {
                action_nonce: Hash([9; 32]),
                ..base.clone()
            },
        ] {
            assert_ne!(variant.tx_final(), h);
        }
    }

    #[test]
    fn action_nonce_is_unique_per_object_set() {
        let n1 = action_nonce(1, &[Hash([1; 32]), Hash([2; 32])]);
        let n2 = action_nonce(1, &[Hash([1; 32]), Hash([3; 32])]);
        let n3 = action_nonce(2, &[Hash([1; 32]), Hash([2; 32])]);
        assert_ne!(n1, n2);
        assert_ne!(n1, n3);
    }

    #[test]
    fn nullifier_helpers_are_consistent() {
        let o = obj_with_key(7);
        let kh = object_key_hash(&o).unwrap();
        let n_via_helper = nullifier_from_key_hash(kh);
        let n_direct = compute_nullifier(&o);
        assert_eq!(n_via_helper, n_direct);
    }

    #[test]
    fn missing_key_means_no_key_hash() {
        let mut o = Object::new();
        o.insert("blueprint", "Log");
        assert!(object_key_hash(&o).is_none());

        // Wrong-typed `key` is also treated as missing.
        let mut o2 = Object::new();
        o2.insert("key", Value::String("not a hash".into()));
        assert!(object_key_hash(&o2).is_none());
    }
}
