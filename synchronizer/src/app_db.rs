//! RocksDB-backed app-state store.
//!
//! Stores the SHA-256 sparse-Merkle-tree nodes used by [`txlib_core`] under
//! `n/{node_hash}` keys. The value layout is `left_hash || right_hash`
//! (64 bytes). Default subtrees aren't stored — any miss is treated as a
//! default subtree at the current depth.
//!
//! `AppDb` is `Clone` (cheap — just an `Arc<TransactionDB>` bump) so multiple
//! consumers (state machine, API server) can share one connection. It also
//! implements [`NodeStore`] directly, which means callers can feed `&app_db`
//! to [`PersistentSmt::open`].

use std::{fmt, sync::Arc};

use anyhow::{Context, Result, anyhow};
use rocksdb::{Options, TransactionDB, TransactionDBOptions};
use txlib_core::Hash;
use txlib_core::merkle::{MerkleProof, leaf_hash};
use txlib_core::merkle_store::{NodeStore, NodeStoreError, PersistentSmt};

use crate::head::CanonicalRoots;

const NODE_PREFIX: &[u8; 2] = b"n/";
const NODE_VALUE_BYTES: usize = 64;

fn node_key(hash: Hash) -> Vec<u8> {
    let mut k = Vec::with_capacity(2 + 32);
    k.extend_from_slice(NODE_PREFIX);
    k.extend_from_slice(hash.as_bytes());
    k
}

#[derive(Clone)]
pub struct AppDb {
    db: Arc<TransactionDB>,
}

impl fmt::Debug for AppDb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "AppDb(path: {:?})", self.db.path())
    }
}

impl AppDb {
    pub fn connect(db_path: &str) -> Result<Self> {
        let mut options = Options::default();
        options.create_if_missing(true);
        let txn_options = TransactionDBOptions::default();
        let inner = TransactionDB::open(&options, &txn_options, db_path)
            .map_err(|err| anyhow!("{err}"))
            .with_context(|| format!("Failed to open RocksDB at path {db_path}"))?;
        Ok(Self {
            db: Arc::new(inner),
        })
    }

    /// Open the persistent transactions SMT at the given root.
    pub fn open_transactions(&self, root: Hash) -> PersistentSmt<'_, AppDb> {
        PersistentSmt::open(root, self)
    }

    /// Open the persistent nullifiers SMT at the given root.
    pub fn open_nullifiers(&self, root: Hash) -> PersistentSmt<'_, AppDb> {
        PersistentSmt::open(root, self)
    }

    /// Membership + inclusion proof for a tx_final under `roots.transactions`.
    /// Returns `(present, proof)` where `proof` verifies against `tx_final`
    /// when present and against `ZERO_HASH` when absent.
    pub fn prove_tx(&self, roots: &CanonicalRoots, tx_hash: Hash) -> Result<(bool, MerkleProof)> {
        let txs = self.open_transactions(roots.transactions);
        let present = txs
            .contains_set_member(tx_hash)
            .map_err(|e| anyhow!("{e}"))?;
        let proof = txs.prove(tx_hash).map_err(|e| anyhow!("{e}"))?;
        Ok((present, proof))
    }

    /// Batch membership for transactions under `roots.transactions`.
    pub fn tx_exists_batch(&self, roots: &CanonicalRoots, tx_hashes: &[Hash]) -> Result<Vec<bool>> {
        let txs = self.open_transactions(roots.transactions);
        tx_hashes
            .iter()
            .map(|hash| {
                txs.contains_set_member(*hash)
                    .map_err(|e| anyhow!("{e}"))
            })
            .collect()
    }

    /// Batch membership for nullifiers under `roots.nullifiers`.
    pub fn nullifier_exists_batch(
        &self,
        roots: &CanonicalRoots,
        nullifiers: &[Hash],
    ) -> Result<Vec<bool>> {
        let nulls = self.open_nullifiers(roots.nullifiers);
        nullifiers
            .iter()
            .map(|hash| nulls.contains_set_member(*hash).map_err(|e| anyhow!("{e}")))
            .collect()
    }

    /// Inclusion proof for a nullifier under `roots.nullifiers`.
    #[allow(dead_code)] // public API for the driver crate (Phase 4)
    pub fn prove_nullifier(
        &self,
        roots: &CanonicalRoots,
        nullifier: Hash,
    ) -> Result<(bool, MerkleProof)> {
        let nulls = self.open_nullifiers(roots.nullifiers);
        let present = nulls
            .contains_set_member(nullifier)
            .map_err(|e| anyhow!("{e}"))?;
        let proof = nulls.prove(nullifier).map_err(|e| anyhow!("{e}"))?;
        Ok((present, proof))
    }
}

impl NodeStore for AppDb {
    fn get(&self, hash: Hash) -> Result<Option<(Hash, Hash)>, NodeStoreError> {
        match self
            .db
            .get(node_key(hash))
            .map_err(NodeStoreError::from_display)?
        {
            None => Ok(None),
            Some(bytes) => {
                if bytes.len() != NODE_VALUE_BYTES {
                    return Err(NodeStoreError::from_display(format!(
                        "node {hash}: expected {NODE_VALUE_BYTES} bytes, got {}",
                        bytes.len()
                    )));
                }
                let mut left = [0u8; 32];
                let mut right = [0u8; 32];
                left.copy_from_slice(&bytes[..32]);
                right.copy_from_slice(&bytes[32..]);
                Ok(Some((Hash(left), Hash(right))))
            }
        }
    }

    fn put(&self, hash: Hash, left: Hash, right: Hash) -> Result<(), NodeStoreError> {
        let mut value = Vec::with_capacity(NODE_VALUE_BYTES);
        value.extend_from_slice(left.as_bytes());
        value.extend_from_slice(right.as_bytes());
        self.db
            .put(node_key(hash), value)
            .map_err(NodeStoreError::from_display)
    }
}

/// 32-byte canonical encoding of a `Hash` for storage in Postgres `BYTEA`.
/// Just the raw bytes of the SHA-256 digest; round-trips via [`db_bytes_to_hash`].
pub fn hash_to_db_bytes(hash: Hash) -> Vec<u8> {
    hash.as_bytes().to_vec()
}

pub fn db_bytes_to_hash(bytes: &[u8]) -> Result<Hash> {
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| anyhow!("invalid hash byte length: {}", bytes.len()))?;
    Ok(Hash(arr))
}

// Re-export for callers that want to verify proofs returned by `prove_tx`,
// `prove_nullifier`, etc.
pub use txlib_core::merkle::verify_inclusion;

/// Verify a Set-membership proof: the leaf at path `key` has value `key`.
#[allow(dead_code)] // public API for the driver crate (Phase 4) + used in tests
pub fn verify_set_membership(root: Hash, key: Hash, proof: &MerkleProof) -> bool {
    verify_inclusion(root, key, key, proof)
}

/// Verify a Set-non-membership proof: the leaf at path `key` has value `ZERO_HASH`.
#[allow(dead_code)] // public API for the driver crate (Phase 4) + used in tests
pub fn verify_set_non_membership(root: Hash, key: Hash, proof: &MerkleProof) -> bool {
    verify_inclusion(root, key, Hash::default(), proof)
}

/// `leaf_hash(value)` exposed for callers that need to interpret raw leaf bytes.
#[allow(dead_code)] // public API for the driver crate (Phase 4)
pub fn leaf_for_value(value: Hash) -> Hash {
    leaf_hash(&value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use txlib_core::hash::sha256;

    fn open_test_db() -> (AppDb, TempDir) {
        let dir = TempDir::new().expect("tempdir");
        let app_db = AppDb::connect(dir.path().to_str().unwrap()).expect("connect");
        (app_db, dir)
    }

    #[test]
    fn test_persistent_tx_membership() {
        let (app_db, _dir) = open_test_db();
        let mut txs = app_db.open_transactions(CanonicalRoots::empty().transactions);
        let tx_hash = sha256(b"test-tx");
        txs.insert(tx_hash, tx_hash).unwrap();
        let new_root = txs.root;

        let roots = CanonicalRoots {
            transactions: new_root,
            ..CanonicalRoots::empty()
        };

        assert_eq!(
            app_db.tx_exists_batch(&roots, &[tx_hash]).unwrap(),
            vec![true]
        );
        let (present, proof) = app_db.prove_tx(&roots, tx_hash).unwrap();
        assert!(present);
        assert!(verify_set_membership(roots.transactions, tx_hash, &proof));
    }

    #[test]
    fn test_proves_non_membership() {
        let (app_db, _dir) = open_test_db();
        let mut txs = app_db.open_transactions(CanonicalRoots::empty().transactions);
        let present = sha256(b"present");
        txs.insert(present, present).unwrap();
        let roots = CanonicalRoots {
            transactions: txs.root,
            ..CanonicalRoots::empty()
        };

        let absent = sha256(b"absent");
        let (present_flag, proof) = app_db.prove_tx(&roots, absent).unwrap();
        assert!(!present_flag);
        assert!(verify_set_non_membership(roots.transactions, absent, &proof));
    }

    #[test]
    fn test_old_root_serves_old_view_after_new_inserts() {
        let (app_db, _dir) = open_test_db();

        let mut txs = app_db.open_transactions(CanonicalRoots::empty().transactions);
        let tx1 = sha256(b"tx1");
        txs.insert(tx1, tx1).unwrap();
        let root1 = txs.root;

        let tx2 = sha256(b"tx2");
        txs.insert(tx2, tx2).unwrap();

        // Reopen at the old root and confirm it still serves the old view.
        let old_roots = CanonicalRoots {
            transactions: root1,
            ..CanonicalRoots::empty()
        };
        assert_eq!(
            app_db.tx_exists_batch(&old_roots, &[tx1, tx2]).unwrap(),
            vec![true, false]
        );
    }

    #[test]
    fn test_db_bytes_roundtrip() {
        let h = sha256(b"x");
        let bytes = hash_to_db_bytes(h);
        assert_eq!(bytes.len(), 32);
        let h2 = db_bytes_to_hash(&bytes).unwrap();
        assert_eq!(h, h2);
    }
}
