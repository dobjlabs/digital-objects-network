use std::{fmt, path::Path, sync::Arc};

use anyhow::{anyhow, Context, Result};
use pod2::{
    backends::plonky2::primitives::merkletree::{self, MerkleProof},
    middleware::{
        containers::{Array, Set},
        db::DB as PodDb,
        Hash, RawValue, Value, EMPTY_HASH, F,
    },
};
use rocksdb::{Options, TransactionDB, TransactionDBOptions};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use txlib::StateRoot;

const META_HEAD_KEY: &[u8] = b"meta/head";

fn node_key(hash: Hash) -> Vec<u8> {
    let mut k = Vec::with_capacity(34);
    k.extend_from_slice(b"n/");
    k.extend_from_slice(&RawValue::from(hash).to_bytes());
    k
}

fn value_key(raw: RawValue) -> Vec<u8> {
    let mut k = Vec::with_capacity(34);
    k.extend_from_slice(b"v/");
    k.extend_from_slice(&raw.to_bytes());
    k
}

/// Compact committed app-state snapshot stored in RocksDB under `meta/head`.
///
/// The actual transaction/nullifier/GSR contents live in persistent POD2 containers;
/// `AppHead` stores the roots and counts used to reopen those containers and serve
/// state/proof queries.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AppHead {
    /// Root of the persistent transactions set.
    pub transactions_root: Hash,
    /// Root of the persistent spent-nullifiers set.
    pub nullifiers_root: Hash,
    /// Root of the prior-GSR array committed inside `txlib::StateRoot`.
    pub state_root_gsrs_root: Hash,
    /// Root of the full GSR history array after appending `current_gsr`.
    pub gsr_history_root: Hash,
    /// Current canonical global state root for this head, if one exists.
    pub current_gsr: Option<Hash>,
    /// Execution block number associated with `current_gsr`.
    pub current_block_number: Option<u32>,
    /// Number of accepted transactions in the canonical state.
    pub tx_count: u64,
    /// Number of spent nullifiers in the canonical state.
    pub nullifier_count: u64,
    /// Number of GSR entries in the persistent history array.
    pub gsr_count: u64,
}

impl Default for AppHead {
    fn default() -> Self {
        Self::empty()
    }
}

impl AppHead {
    pub fn empty() -> Self {
        Self {
            transactions_root: EMPTY_HASH,
            nullifiers_root: EMPTY_HASH,
            state_root_gsrs_root: EMPTY_HASH,
            gsr_history_root: EMPTY_HASH,
            current_gsr: None,
            current_block_number: None,
            tx_count: 0,
            nullifier_count: 0,
            gsr_count: 0,
        }
    }

    pub fn current_state_root(&self) -> Option<StateRoot> {
        self.current_block_number.map(|block_number| {
            StateRoot::new(
                block_number as i64,
                self.transactions_root,
                self.nullifiers_root,
                self.state_root_gsrs_root,
            )
        })
    }
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
        let db = Self::open(db_path)
            .with_context(|| format!("Failed to open RocksDB at path {db_path}"))?;
        if db.load_head_meta()?.is_none() {
            db.store_head_meta(&AppHead::empty())?;
        }
        Ok(db)
    }

    fn open(path: impl AsRef<Path>) -> Result<Self> {
        let mut options = Options::default();
        options.create_if_missing(true);
        let txn_options = TransactionDBOptions::default();
        let inner =
            TransactionDB::open(&options, &txn_options, path).map_err(|err| anyhow!("{err}"))?;
        Ok(Self {
            db: Arc::new(inner),
        })
    }

    fn load_meta_json<T: DeserializeOwned>(&self, key: &[u8]) -> Result<Option<T>> {
        match self.db.get(key)? {
            None => Ok(None),
            Some(bytes) => Ok(Some(serde_json::from_slice(bytes.as_ref())?)),
        }
    }

    fn store_meta_json<T: Serialize>(&self, key: &[u8], value: &T) -> Result<()> {
        let bytes = serde_json::to_vec(value)?;
        self.db.put(key, bytes).map_err(|err| anyhow!("{err}"))
    }

    fn load_head_meta(&self) -> Result<Option<AppHead>> {
        self.load_meta_json(META_HEAD_KEY)
    }

    fn store_head_meta(&self, head: &AppHead) -> Result<()> {
        self.store_meta_json(META_HEAD_KEY, head)
    }

    pub fn load_head(&self) -> Result<AppHead> {
        self.load_head_meta()?
            .context("app state missing meta/head after initialization")
    }

    pub fn store_head(&self, head: &AppHead) -> Result<()> {
        self.store_head_meta(head)
    }

    pub fn open_transactions(&self, root: Hash) -> Result<Set> {
        Ok(Set::from_db(root, self.db_box())?)
    }

    pub fn open_nullifiers(&self, root: Hash) -> Result<Set> {
        Ok(Set::from_db(root, self.db_box())?)
    }

    pub fn open_gsr_history(&self, root: Hash) -> Result<Array> {
        Ok(Array::from_db(root, self.db_box())?)
    }

    pub fn prove_tx(&self, head: &AppHead, tx_hash: Hash) -> Result<(bool, MerkleProof)> {
        let txs = self.open_transactions(head.transactions_root)?;
        let value = Value::from(tx_hash);
        match txs.prove(&value) {
            Ok(proof) => Ok((true, proof)),
            Err(_) => Ok((false, txs.prove_nonexistence(&value)?)),
        }
    }

    pub fn tx_exists_batch(&self, head: &AppHead, tx_hashes: &[Hash]) -> Result<Vec<bool>> {
        let txs = self.open_transactions(head.transactions_root)?;
        tx_hashes
            .iter()
            .map(|hash| {
                txs.contains(&Value::from(*hash))
                    .map_err(|err| anyhow!("{err}"))
            })
            .collect()
    }

    pub fn nullifier_exists_batch(&self, head: &AppHead, nullifiers: &[Hash]) -> Result<Vec<bool>> {
        let nullifier_set = self.open_nullifiers(head.nullifiers_root)?;
        nullifiers
            .iter()
            .map(|hash| {
                nullifier_set
                    .contains(&Value::from(*hash))
                    .map_err(|err| anyhow!("{err}"))
            })
            .collect()
    }

    fn db_box(&self) -> Box<dyn PodDb> {
        Box::new(self.clone())
    }
}

impl merkletree::db::DB for AppDb {
    fn load_node(&self, hash: Hash) -> Result<Option<merkletree::Node>> {
        if hash == EMPTY_HASH {
            return Ok(Some(merkletree::Node::Intermediate(
                merkletree::Intermediate::new(EMPTY_HASH, EMPTY_HASH),
            )));
        }

        match self.db.get(node_key(hash))? {
            None => Ok(None),
            Some(bytes) => Ok(Some(merkletree::Node::decode(bytes.as_ref())?)),
        }
    }

    fn store_node(&mut self, node: merkletree::Node) -> Result<()> {
        self.db
            .put(node_key(node.hash()), node.encode()?)
            .map_err(|err| anyhow!("rocksdb transaction put failed: {err}"))
    }
}

impl PodDb for AppDb {
    fn load_value(&self, raw: RawValue) -> anyhow::Result<Option<Value>> {
        match self.db.get(value_key(raw))? {
            None => Ok(None),
            Some(bytes) => Ok(Some({
                if bytes.is_empty() {
                    Value::from(raw)
                } else {
                    Value::from_bytes(bytes.as_ref(), self.clone_box())?
                }
            })),
        }
    }

    fn store_value(&mut self, value: Value) -> anyhow::Result<()> {
        let value_key = value_key(value.raw());
        let tx = self.db.transaction();
        if let Some(old_value_bytes) = tx.get_for_update(&value_key, true)? {
            let is_raw = old_value_bytes.is_empty();
            // If we had a non-RawValue stored don't overwrite it (specially not with a
            // RawValue).   Also skip redundant RawValue overwrite.
            if !is_raw || value.is_raw() {
                return Ok(());
            }
        }
        let value_bytes = if value.is_raw() {
            // For RawValue we store an empty vector because it's a duplicate of the key.
            // This way we can easily check for RawValue without decoding.
            vec![]
        } else {
            Value::to_bytes(&value)
        };
        tx.put(value_key, value_bytes)?;
        Ok(tx.commit()?)
    }

    fn is_persistent(&self) -> bool {
        true
    }

    fn clone_box(&self) -> Box<dyn PodDb> {
        Box::new(self.clone())
    }
}

pub fn hash_to_db_bytes(hash: Hash) -> Vec<u8> {
    RawValue::from(hash).to_bytes().to_vec()
}

pub fn db_bytes_to_hash(bytes: &[u8]) -> Result<Hash> {
    let limbs: [[u8; 8]; 4] = bytes
        .chunks_exact(8)
        .map(|chunk| {
            chunk
                .try_into()
                .map_err(|_| anyhow!("invalid hash limb length"))
        })
        .collect::<Result<Vec<[u8; 8]>>>()?
        .try_into()
        .map_err(|_| anyhow!("invalid hash byte length"))?;

    Ok(Hash(limbs.map(|limb| F(u64::from_le_bytes(limb)))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex::FromHex;
    use pod2::middleware::{Value, EMPTY_HASH};
    use tempfile::TempDir;

    fn test_hash(byte: u8) -> Hash {
        Hash::from_hex(hex::encode([byte; 32])).expect("valid test hash")
    }

    fn open_test_db() -> (AppDb, TempDir) {
        let dir = TempDir::new().expect("tempdir");
        let app_db = AppDb::connect(dir.path().to_str().unwrap()).expect("connect");
        (app_db, dir)
    }

    #[test]
    fn test_bootstraps_empty_head() {
        let (app_db, _dir) = open_test_db();
        assert_eq!(app_db.load_head().unwrap(), AppHead::empty());
    }

    #[test]
    fn test_store_and_reload_head() {
        let (app_db, _dir) = open_test_db();
        let head = AppHead {
            transactions_root: test_hash(1),
            nullifiers_root: test_hash(2),
            state_root_gsrs_root: test_hash(3),
            gsr_history_root: test_hash(4),
            current_gsr: Some(test_hash(5)),
            current_block_number: Some(7),
            tx_count: 9,
            nullifier_count: 11,
            gsr_count: 13,
        };
        app_db.store_head(&head).unwrap();
        assert_eq!(app_db.load_head().unwrap(), head);
    }

    #[test]
    fn test_persistent_tx_membership() {
        let (app_db, _dir) = open_test_db();
        let mut txs = app_db.open_transactions(EMPTY_HASH).unwrap();
        let tx_hash = test_hash(9);
        txs.insert(&Value::from(tx_hash)).unwrap();

        let head = AppHead {
            transactions_root: txs.commitment(),
            ..AppHead::empty()
        };

        assert_eq!(
            app_db.tx_exists_batch(&head, &[tx_hash]).unwrap(),
            vec![true]
        );
        let (present, _proof) = app_db.prove_tx(&head, tx_hash).unwrap();
        assert!(present);
    }
}
