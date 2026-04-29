//! Sparse Merkle tree with depth 256 over SHA-256.
//!
//! - **Verifier** (no_std, used by the guest): [`verify_inclusion`].
//! - **Builder** (in-memory, host-only): [`MerkleTree`] for tests and the
//!   reference synchronizer integration. Phase 2 will swap in a persistent,
//!   incremental SMT backed by RocksDB; this in-memory version makes the
//!   verifier easy to test against and exercises the same hash recipe.
//!
//! ## Key bit convention
//! `bit_at(key, i)` reads the `i`-th most significant bit of the 32-byte key
//! (so `i=0` is the MSB of `key[0]`, and `i=255` is the LSB of `key[31]`).
//! Following the standard SMT convention, bit `i` selects the child at depth
//! `i+1` while descending from the root.
//!
//! ## Leaf / node hashes
//! ```text
//! leaf(v)        = SHA256( DOBJ-SLF || v )
//! node(l, r)     = SHA256( DOBJ-SND || l || r )
//! default[0]     = leaf( ZERO_HASH )
//! default[s]     = node( default[s-1], default[s-1] )
//! empty_root     = default[SMT_DEPTH]
//! ```
//! `default[s]` is the root of an all-empty subtree whose leaves are `s`
//! levels below — leaf hashes are position-independent, which keeps the
//! default-subtree precomputation trivial (no per-key default).
//!
//! ## Set semantics
//! For a set of hashes (transactions, nullifiers), the convention is:
//! **leaf at path `H` has value `H` when present, `ZERO_HASH` when absent.**
//! Membership is `verify_inclusion(root, key=H, value=H, proof)`;
//! non-membership is the same call with `value=ZERO_HASH`.

use alloc::vec::Vec;
use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

use crate::hash::{Hash, domain, sha256_concat};
#[cfg(any(test, feature = "host"))]
use crate::hash::ZERO_HASH;

pub const SMT_DEPTH: usize = 256;

/// Position-independent leaf hash. The path through the tree is determined
/// by the key's bits; the leaf hash itself only needs to commit to the value.
pub fn leaf_hash(value: &Hash) -> Hash {
    sha256_concat(&[domain::SMT_LEAF, value.as_bytes()])
}

pub fn node_hash(left: &Hash, right: &Hash) -> Hash {
    sha256_concat(&[domain::SMT_NODE, left.as_bytes(), right.as_bytes()])
}

/// Read the `i`-th most significant bit (MSB-first) of a 32-byte key.
pub fn bit_at(key: &Hash, i: usize) -> u8 {
    debug_assert!(i < SMT_DEPTH);
    (key.0[i / 8] >> (7 - (i % 8))) & 1
}

/// Inclusion proof for a leaf at path `key`. `siblings[i]` is the sibling at
/// depth `i + 1`, so verification walks the array in reverse from leaf to root.
#[derive(
    Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub struct MerkleProof {
    pub siblings: Vec<Hash>,
}

impl MerkleProof {
    pub fn is_well_formed(&self) -> bool {
        self.siblings.len() == SMT_DEPTH
    }
}

/// Verify that the leaf at path `key` has value `value` under `root`.
/// Pass `value = ZERO_HASH` to verify non-membership.
pub fn verify_inclusion(root: Hash, key: Hash, value: Hash, proof: &MerkleProof) -> bool {
    if !proof.is_well_formed() {
        return false;
    }
    let mut current = leaf_hash(&value);
    for i in (0..SMT_DEPTH).rev() {
        let bit = bit_at(&key, i);
        let sibling = &proof.siblings[i];
        current = if bit == 0 {
            node_hash(&current, sibling)
        } else {
            node_hash(sibling, &current)
        };
    }
    current == root
}

// ---------------------------------------------------------------------------
// In-memory builder (host-only)
// ---------------------------------------------------------------------------

#[cfg(feature = "host")]
pub use host::MerkleTree;

#[cfg(feature = "host")]
mod host {
    use super::*;
    use alloc::collections::BTreeMap;
    use alloc::vec;

    /// In-memory sparse Merkle tree. Suitable for tests and small data sets;
    /// the synchronizer will use a persistent variant in Phase 2.
    ///
    /// Each `root()` / `prove()` call recomputes from scratch in `O(n · depth)`
    /// time. Don't use for large sets.
    pub struct MerkleTree {
        leaves: BTreeMap<Hash, Hash>,
        default_subtree: Vec<Hash>,
    }

    impl MerkleTree {
        pub fn new() -> Self {
            let mut default_subtree = vec![ZERO_HASH; SMT_DEPTH + 1];
            default_subtree[0] = leaf_hash(&ZERO_HASH);
            for s in 1..=SMT_DEPTH {
                let prev = default_subtree[s - 1];
                default_subtree[s] = node_hash(&prev, &prev);
            }
            Self {
                leaves: BTreeMap::new(),
                default_subtree,
            }
        }

        pub fn empty_root(&self) -> Hash {
            self.default_subtree[SMT_DEPTH]
        }

        pub fn insert(&mut self, key: Hash, value: Hash) {
            if value.is_zero() {
                self.leaves.remove(&key);
            } else {
                self.leaves.insert(key, value);
            }
        }

        pub fn get(&self, key: &Hash) -> Hash {
            self.leaves.get(key).copied().unwrap_or(ZERO_HASH)
        }

        pub fn root(&self) -> Hash {
            let leaves: Vec<(Hash, Hash)> =
                self.leaves.iter().map(|(k, v)| (*k, *v)).collect();
            self.compute_subtree(&leaves, 0)
        }

        /// Build an inclusion proof for `key`. The returned proof verifies
        /// against the value currently at that path (or `ZERO_HASH` if absent).
        pub fn prove(&self, key: Hash) -> MerkleProof {
            let leaves: Vec<(Hash, Hash)> =
                self.leaves.iter().map(|(k, v)| (*k, *v)).collect();
            let mut siblings = Vec::with_capacity(SMT_DEPTH);
            let mut path = leaves.as_slice();
            for d in 0..SMT_DEPTH {
                let pivot = path.partition_point(|(k, _)| bit_at(k, d) == 0);
                let (left, right) = path.split_at(pivot);
                let bit = bit_at(&key, d);
                let (next, sibling_leaves) = if bit == 0 {
                    (left, right)
                } else {
                    (right, left)
                };
                siblings.push(self.compute_subtree(sibling_leaves, d + 1));
                path = next;
            }
            MerkleProof { siblings }
        }

        /// Compute the root of a subtree at `depth_from_root`. `leaves` are
        /// all (sorted) leaves under this subtree.
        fn compute_subtree(
            &self,
            leaves: &[(Hash, Hash)],
            depth_from_root: usize,
        ) -> Hash {
            if leaves.is_empty() {
                return self.default_subtree[SMT_DEPTH - depth_from_root];
            }
            if depth_from_root == SMT_DEPTH {
                // Exactly one leaf, by construction (paths are unique).
                debug_assert_eq!(leaves.len(), 1);
                return leaf_hash(&leaves[0].1);
            }
            let pivot = leaves.partition_point(|(k, _)| bit_at(k, depth_from_root) == 0);
            let (left, right) = leaves.split_at(pivot);
            let l = self.compute_subtree(left, depth_from_root + 1);
            let r = self.compute_subtree(right, depth_from_root + 1);
            node_hash(&l, &r)
        }
    }

    impl Default for MerkleTree {
        fn default() -> Self {
            Self::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::sha256;

    #[test]
    fn bit_at_works() {
        let mut k = Hash([0u8; 32]);
        k.0[0] = 0b1010_0000;
        assert_eq!(bit_at(&k, 0), 1);
        assert_eq!(bit_at(&k, 1), 0);
        assert_eq!(bit_at(&k, 2), 1);
        assert_eq!(bit_at(&k, 3), 0);
        k.0[31] = 0b0000_0001;
        assert_eq!(bit_at(&k, 255), 1);
        assert_eq!(bit_at(&k, 254), 0);
    }

    #[test]
    fn empty_tree_has_stable_root() {
        let t1 = MerkleTree::new();
        let t2 = MerkleTree::new();
        assert_eq!(t1.root(), t2.root());
        assert_eq!(t1.root(), t1.empty_root());
    }

    #[test]
    fn single_insert_changes_root() {
        let mut t = MerkleTree::new();
        let empty = t.root();
        t.insert(sha256(b"k1"), sha256(b"v1"));
        assert_ne!(t.root(), empty);
    }

    #[test]
    fn membership_proof_verifies() {
        let mut t = MerkleTree::new();
        let key = sha256(b"hello");
        let value = sha256(b"world");
        t.insert(key, value);
        let proof = t.prove(key);
        assert!(proof.is_well_formed());
        assert!(verify_inclusion(t.root(), key, value, &proof));
    }

    #[test]
    fn membership_proof_rejects_wrong_value() {
        let mut t = MerkleTree::new();
        let key = sha256(b"k");
        let value = sha256(b"v");
        t.insert(key, value);
        let proof = t.prove(key);
        let bad_value = sha256(b"v'");
        assert!(!verify_inclusion(t.root(), key, bad_value, &proof));
    }

    #[test]
    fn membership_proof_rejects_wrong_root() {
        let mut t = MerkleTree::new();
        let key = sha256(b"k");
        let value = sha256(b"v");
        t.insert(key, value);
        let proof = t.prove(key);
        let bad_root = sha256(b"not-the-root");
        assert!(!verify_inclusion(bad_root, key, value, &proof));
    }

    #[test]
    fn non_membership_proof_verifies() {
        let mut t = MerkleTree::new();
        t.insert(sha256(b"present"), sha256(b"value"));
        let absent = sha256(b"absent");
        let proof = t.prove(absent);
        assert!(verify_inclusion(t.root(), absent, ZERO_HASH, &proof));
    }

    #[test]
    fn many_insertions() {
        let mut t = MerkleTree::new();
        let mut keys = Vec::new();
        for i in 0..16u32 {
            let k = sha256(&i.to_le_bytes());
            t.insert(k, k); // set semantics: value = key
            keys.push(k);
        }
        let root = t.root();
        for k in &keys {
            let proof = t.prove(*k);
            assert!(verify_inclusion(root, *k, *k, &proof), "missing key {k:?}");
        }
        // A key not in the set produces a non-membership proof.
        let absent = sha256(b"definitely not there");
        let proof = t.prove(absent);
        assert!(verify_inclusion(root, absent, ZERO_HASH, &proof));
        // And asserting membership of an absent key with a fake value fails.
        assert!(!verify_inclusion(root, absent, absent, &proof));
    }

    #[test]
    fn insert_zero_removes() {
        let mut t = MerkleTree::new();
        let k = sha256(b"removable");
        let v = sha256(b"v");
        t.insert(k, v);
        let root_with = t.root();
        t.insert(k, ZERO_HASH);
        let root_without = t.root();
        assert_eq!(root_without, t.empty_root());
        assert_ne!(root_with, root_without);
    }

    #[test]
    fn proof_is_correct_length() {
        let mut t = MerkleTree::new();
        t.insert(sha256(b"k"), sha256(b"v"));
        let proof = t.prove(sha256(b"k"));
        assert_eq!(proof.siblings.len(), SMT_DEPTH);
    }

    #[test]
    fn malformed_proof_rejected() {
        let bad = MerkleProof { siblings: vec![] };
        assert!(!verify_inclusion(ZERO_HASH, ZERO_HASH, ZERO_HASH, &bad));
    }
}
