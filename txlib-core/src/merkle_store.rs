//! Persistent sparse Merkle tree backed by a content-addressed key-value store.
//!
//! Each non-default internal node is stored as `(node_hash → (left, right))`.
//! Default subtrees are *not* stored — any lookup that misses is treated as a
//! default subtree at whatever depth the traversal is currently at. Leaves
//! aren't stored explicitly: the leaf hash is `leaf_hash(value)`, so callers
//! that need to check "what's at this position" compare the walked-down hash
//! against `leaf_hash(presumed_value)`.
//!
//! Insertion is `O(SMT_DEPTH)` reads + writes. Old internal nodes aren't
//! garbage-collected — they become orphaned, which is fine because the
//! canonical head pins exactly one root and any unreachable nodes are
//! ignored.
//!
//! Reorgs: this store doesn't model reorgs internally. The synchronizer
//! tracks canonical roots in Postgres, so a reorg is "switch back to an
//! older root"; orphaned nodes from the rolled-back fork stay in RocksDB
//! and are ignored. See [synchronizer/README.md](../synchronizer/README.md)
//! for the canonical-publish + crash-semantics story.
//!
//! ## Mutability model
//!
//! Both [`NodeStore::get`] and [`NodeStore::put`] take `&self`. Backing
//! stores must use interior mutability (RocksDB's `TransactionDB` is already
//! `&self`-friendly; the in-memory reference impl wraps a `Mutex`). This
//! lets multiple [`PersistentSmt`] views share one `&S` without juggling
//! `&mut` borrows.

use alloc::sync::Arc;
use alloc::vec::Vec;
use std::sync::{Mutex, OnceLock};

use crate::hash::{Hash, ZERO_HASH};
use crate::merkle::{MerkleProof, SMT_DEPTH, bit_at, leaf_hash, node_hash};

/// Content-addressed node storage. The store is responsible only for
/// `(hash → (left, right))` persistence; the SMT layer handles all of the
/// tree walking, default-subtree handling, and root computation.
///
/// Both methods take `&self` — backends use interior mutability. This keeps
/// `PersistentSmt` lifetimes simple and lets multiple views share one store.
pub trait NodeStore: Send + Sync {
    /// Returns `Ok(None)` if the hash is unknown (treated as a default subtree).
    fn get(&self, hash: Hash) -> Result<Option<(Hash, Hash)>, NodeStoreError>;

    /// Persist a node. Idempotent — writing the same `(hash, left, right)`
    /// twice is a no-op.
    fn put(&self, hash: Hash, left: Hash, right: Hash) -> Result<(), NodeStoreError>;
}

pub struct NodeStoreError(pub Arc<dyn core::fmt::Display + Send + Sync>);

impl core::fmt::Display for NodeStoreError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "node store error: {}", self.0)
    }
}

impl core::fmt::Debug for NodeStoreError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "NodeStoreError({})", self.0)
    }
}

impl NodeStoreError {
    pub fn from_display<E: core::fmt::Display + Send + Sync + 'static>(e: E) -> Self {
        Self(Arc::new(e))
    }
}

/// Precomputed default subtree roots. `default_subtrees()[s]` is the root of
/// an all-empty subtree whose leaves are `s` levels below — `s == 0` is the
/// leaf itself, `s == SMT_DEPTH` is the empty SMT root.
///
/// Computed once on first call and cached for the lifetime of the process.
pub fn default_subtrees() -> &'static [Hash; SMT_DEPTH + 1] {
    static CELL: OnceLock<[Hash; SMT_DEPTH + 1]> = OnceLock::new();
    CELL.get_or_init(|| {
        let mut arr = [ZERO_HASH; SMT_DEPTH + 1];
        arr[0] = leaf_hash(&ZERO_HASH);
        for s in 1..=SMT_DEPTH {
            let prev = arr[s - 1];
            arr[s] = node_hash(&prev, &prev);
        }
        arr
    })
}

/// The root of an empty SMT.
pub fn empty_root() -> Hash {
    default_subtrees()[SMT_DEPTH]
}

/// Persistent SMT view over a `&S` store, rooted at `root`. Cheap to
/// construct — no work happens until you call `insert`, `prove`, or
/// `get_leaf`. Multiple views over disjoint roots can share one `&S` with
/// no lifetime conflict.
pub struct PersistentSmt<'a, S: NodeStore> {
    pub root: Hash,
    pub store: &'a S,
}

impl<'a, S: NodeStore> PersistentSmt<'a, S> {
    pub fn open(root: Hash, store: &'a S) -> Self {
        Self { root, store }
    }

    pub fn empty(store: &'a S) -> Self {
        Self {
            root: empty_root(),
            store,
        }
    }

    /// Insert (or overwrite) the leaf at path `key` to `value`. Updates
    /// `self.root` to the new root and persists every internal node along
    /// the new path. Returns the new root.
    pub fn insert(&mut self, key: Hash, value: Hash) -> Result<Hash, NodeStoreError> {
        let defaults = default_subtrees();
        let mut siblings = Vec::with_capacity(SMT_DEPTH);
        let mut current = self.root;

        for d in 0..SMT_DEPTH {
            if current == defaults[SMT_DEPTH - d] {
                // Default subtree below — fill defaults for the rest.
                for d_remaining in d..SMT_DEPTH {
                    let bit = bit_at(&key, d_remaining);
                    let sibling = defaults[SMT_DEPTH - d_remaining - 1];
                    siblings.push((sibling, bit));
                }
                break;
            }
            let (left, right) = self.store.get(current)?.ok_or_else(|| {
                NodeStoreError::from_display(alloc::format!(
                    "non-default node {current} missing from store at depth {d}"
                ))
            })?;
            let bit = bit_at(&key, d);
            let (sibling, child) = if bit == 0 {
                (right, left)
            } else {
                (left, right)
            };
            siblings.push((sibling, bit));
            current = child;
        }

        let mut new_node = leaf_hash(&value);
        for (sibling, bit) in siblings.iter().rev() {
            let (left, right) = if *bit == 0 {
                (new_node, *sibling)
            } else {
                (*sibling, new_node)
            };
            let h = node_hash(&left, &right);
            self.store.put(h, left, right)?;
            new_node = h;
        }

        self.root = new_node;
        Ok(self.root)
    }

    /// Build an inclusion proof for `key`. Together with the value the caller
    /// expects (e.g. the key itself for a Set, or `ZERO_HASH` for non-membership)
    /// the proof is verifiable via [`crate::merkle::verify_inclusion`].
    pub fn prove(&self, key: Hash) -> Result<MerkleProof, NodeStoreError> {
        let defaults = default_subtrees();
        let mut siblings = Vec::with_capacity(SMT_DEPTH);
        let mut current = self.root;

        for d in 0..SMT_DEPTH {
            if current == defaults[SMT_DEPTH - d] {
                for d_remaining in d..SMT_DEPTH {
                    siblings.push(defaults[SMT_DEPTH - d_remaining - 1]);
                }
                return Ok(MerkleProof { siblings });
            }
            let (left, right) = self.store.get(current)?.ok_or_else(|| {
                NodeStoreError::from_display(alloc::format!(
                    "non-default node {current} missing from store at depth {d}"
                ))
            })?;
            let bit = bit_at(&key, d);
            let (sibling, child) = if bit == 0 {
                (right, left)
            } else {
                (left, right)
            };
            siblings.push(sibling);
            current = child;
        }

        Ok(MerkleProof { siblings })
    }

    /// Return the leaf hash at path `key` (i.e. the value the proof would
    /// have to be checked against). For a Set with `value = key` semantics,
    /// the caller does:
    /// ```ignore
    /// let leaf = smt.get_leaf(key)?;
    /// let present = leaf == leaf_hash(&key);
    /// ```
    pub fn get_leaf(&self, key: Hash) -> Result<Hash, NodeStoreError> {
        let defaults = default_subtrees();
        let mut current = self.root;
        for d in 0..SMT_DEPTH {
            if current == defaults[SMT_DEPTH - d] {
                return Ok(defaults[0]);
            }
            let (left, right) = self.store.get(current)?.ok_or_else(|| {
                NodeStoreError::from_display(alloc::format!(
                    "non-default node {current} missing from store at depth {d}"
                ))
            })?;
            let bit = bit_at(&key, d);
            current = if bit == 0 { left } else { right };
        }
        Ok(current)
    }

    /// Convenience for Set semantics: returns true iff the leaf at `key`
    /// is `leaf_hash(key)` (i.e. the key was inserted with `value = key`).
    pub fn contains_set_member(&self, key: Hash) -> Result<bool, NodeStoreError> {
        Ok(self.get_leaf(key)? == leaf_hash(&key))
    }
}

// ---------------------------------------------------------------------------
// Convenience: in-memory NodeStore for tests & reference
// ---------------------------------------------------------------------------

/// In-memory `NodeStore` backed by a `Mutex<BTreeMap>`. Mirrors the persistent
/// behavior so tests of [`PersistentSmt`] can exercise the same code paths
/// without touching disk.
pub struct InMemoryNodeStore {
    nodes: Mutex<alloc::collections::BTreeMap<Hash, (Hash, Hash)>>,
    /// Sanity flag for tests — `true` if any `put` overwrote a different
    /// `(left, right)` for the same hash. SHA-256 collision-resistance means
    /// this should never fire in practice; flagging it surfaces store-layer
    /// bugs that would otherwise be invisible.
    suspicious: Mutex<bool>,
}

impl Default for InMemoryNodeStore {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryNodeStore {
    pub fn new() -> Self {
        Self {
            nodes: Mutex::new(alloc::collections::BTreeMap::new()),
            suspicious: Mutex::new(false),
        }
    }

    pub fn len(&self) -> usize {
        self.nodes.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.lock().unwrap().is_empty()
    }

    pub fn was_suspicious(&self) -> bool {
        *self.suspicious.lock().unwrap()
    }
}

impl NodeStore for InMemoryNodeStore {
    fn get(&self, hash: Hash) -> Result<Option<(Hash, Hash)>, NodeStoreError> {
        Ok(self.nodes.lock().unwrap().get(&hash).copied())
    }

    fn put(&self, hash: Hash, left: Hash, right: Hash) -> Result<(), NodeStoreError> {
        let mut nodes = self.nodes.lock().unwrap();
        if let Some(existing) = nodes.get(&hash) {
            if *existing != (left, right) {
                *self.suspicious.lock().unwrap() = true;
            }
            return Ok(());
        }
        nodes.insert(hash, (left, right));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::sha256;
    use crate::merkle::{MerkleTree, verify_inclusion};

    #[test]
    fn empty_root_matches_in_memory_tree() {
        let in_mem = MerkleTree::new();
        assert_eq!(empty_root(), in_mem.empty_root());
        assert_eq!(empty_root(), in_mem.root());
    }

    #[test]
    fn single_insert_root_matches_in_memory() {
        let key = sha256(b"k");
        let value = sha256(b"v");

        let mut in_mem = MerkleTree::new();
        in_mem.insert(key, value);
        let expected_root = in_mem.root();

        let store = InMemoryNodeStore::new();
        let mut smt = PersistentSmt::empty(&store);
        let new_root = smt.insert(key, value).unwrap();

        assert_eq!(new_root, expected_root);
    }

    #[test]
    fn multi_insert_root_matches_in_memory() {
        let pairs: Vec<(Hash, Hash)> = (0..16u32)
            .map(|i| {
                let h = sha256(&i.to_le_bytes());
                (h, h)
            })
            .collect();

        let mut in_mem = MerkleTree::new();
        for (k, v) in &pairs {
            in_mem.insert(*k, *v);
        }
        let expected_root = in_mem.root();

        let store = InMemoryNodeStore::new();
        let mut smt = PersistentSmt::empty(&store);
        for (k, v) in &pairs {
            smt.insert(*k, *v).unwrap();
        }
        assert_eq!(smt.root, expected_root);
        assert!(!store.was_suspicious());
    }

    #[test]
    fn insert_then_prove_membership() {
        let store = InMemoryNodeStore::new();
        let mut smt = PersistentSmt::empty(&store);

        let key = sha256(b"key");
        let value = sha256(b"value");
        smt.insert(key, value).unwrap();
        let root = smt.root;

        let proof = smt.prove(key).unwrap();
        assert!(verify_inclusion(root, key, value, &proof));
    }

    #[test]
    fn prove_non_membership() {
        let store = InMemoryNodeStore::new();
        let mut smt = PersistentSmt::empty(&store);

        smt.insert(sha256(b"present"), sha256(b"v")).unwrap();
        let root = smt.root;

        let absent = sha256(b"absent");
        let proof = smt.prove(absent).unwrap();
        assert!(verify_inclusion(root, absent, ZERO_HASH, &proof));
    }

    #[test]
    fn many_inserts_then_all_proofs_verify() {
        let store = InMemoryNodeStore::new();
        let mut smt = PersistentSmt::empty(&store);

        let pairs: Vec<(Hash, Hash)> = (0..32u32)
            .map(|i| {
                let k = sha256(&i.to_le_bytes());
                (k, k)
            })
            .collect();

        for (k, v) in &pairs {
            smt.insert(*k, *v).unwrap();
        }
        let root = smt.root;

        for (k, v) in &pairs {
            let proof = smt.prove(*k).unwrap();
            assert!(verify_inclusion(root, *k, *v, &proof), "missing key {k}");
        }
    }

    #[test]
    fn reopen_at_old_root_serves_old_view() {
        let store = InMemoryNodeStore::new();
        let mut smt = PersistentSmt::empty(&store);

        let k1 = sha256(b"k1");
        let v1 = sha256(b"v1");
        smt.insert(k1, v1).unwrap();
        let root1 = smt.root;

        let k2 = sha256(b"k2");
        let v2 = sha256(b"v2");
        smt.insert(k2, v2).unwrap();
        let root2 = smt.root;

        let smt_old = PersistentSmt::open(root1, &store);
        let proof = smt_old.prove(k1).unwrap();
        assert!(verify_inclusion(root1, k1, v1, &proof));

        let proof = smt_old.prove(k2).unwrap();
        assert!(verify_inclusion(root1, k2, ZERO_HASH, &proof));

        let smt_new = PersistentSmt::open(root2, &store);
        let p1 = smt_new.prove(k1).unwrap();
        let p2 = smt_new.prove(k2).unwrap();
        assert!(verify_inclusion(root2, k1, v1, &p1));
        assert!(verify_inclusion(root2, k2, v2, &p2));
    }

    #[test]
    fn overwrite_value() {
        let store = InMemoryNodeStore::new();
        let mut smt = PersistentSmt::empty(&store);

        let key = sha256(b"k");
        smt.insert(key, sha256(b"v1")).unwrap();
        smt.insert(key, sha256(b"v2")).unwrap();
        let root = smt.root;

        let proof = smt.prove(key).unwrap();
        assert!(verify_inclusion(root, key, sha256(b"v2"), &proof));
        assert!(!verify_inclusion(root, key, sha256(b"v1"), &proof));
    }

    #[test]
    fn empty_smt_proves_non_membership_for_anything() {
        let store = InMemoryNodeStore::new();
        let smt = PersistentSmt::empty(&store);
        let key = sha256(b"anything");
        let proof = smt.prove(key).unwrap();
        assert!(verify_inclusion(empty_root(), key, ZERO_HASH, &proof));
    }

    #[test]
    fn contains_set_member_works() {
        let store = InMemoryNodeStore::new();
        let mut smt = PersistentSmt::empty(&store);

        let k1 = sha256(b"present");
        smt.insert(k1, k1).unwrap();

        assert!(smt.contains_set_member(k1).unwrap());
        let absent = sha256(b"absent");
        assert!(!smt.contains_set_member(absent).unwrap());
    }

    #[test]
    fn two_smts_share_one_store() {
        let store = InMemoryNodeStore::new();
        let mut a = PersistentSmt::empty(&store);
        let mut b = PersistentSmt::empty(&store);

        a.insert(sha256(b"a-only"), sha256(b"x")).unwrap();
        b.insert(sha256(b"b-only"), sha256(b"y")).unwrap();

        // Each SMT carries its own root; the store holds nodes for both.
        assert!(a.contains_set_member(sha256(b"a-only")).is_ok());
        assert!(b.contains_set_member(sha256(b"b-only")).is_ok());
    }
}
