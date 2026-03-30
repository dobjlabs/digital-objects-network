use std::{fmt, sync::Arc};

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

use crate::head::CanonicalRoots;

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

    pub fn open_transactions(&self, root: Hash) -> Result<Set> {
        Ok(Set::from_db(root, self.db_box())?)
    }

    pub fn open_nullifiers(&self, root: Hash) -> Result<Set> {
        Ok(Set::from_db(root, self.db_box())?)
    }

    pub fn open_gsr_history(&self, root: Hash) -> Result<Array> {
        Ok(Array::from_db(root, self.db_box())?)
    }

    pub fn prove_tx(&self, roots: &CanonicalRoots, tx_hash: Hash) -> Result<(bool, MerkleProof)> {
        let txs = self.open_transactions(roots.transactions)?;
        let value = Value::from(tx_hash);
        match txs.prove(&value) {
            Ok(proof) => Ok((true, proof)),
            Err(_) => Ok((false, txs.prove_nonexistence(&value)?)),
        }
    }

    pub fn tx_exists_batch(&self, roots: &CanonicalRoots, tx_hashes: &[Hash]) -> Result<Vec<bool>> {
        let txs = self.open_transactions(roots.transactions)?;
        tx_hashes
            .iter()
            .map(|hash| {
                txs.contains(&Value::from(*hash))
                    .map_err(|err| anyhow!("{err}"))
            })
            .collect()
    }

    pub fn nullifier_exists_batch(
        &self,
        roots: &CanonicalRoots,
        nullifiers: &[Hash],
    ) -> Result<Vec<bool>> {
        let nullifier_set = self.open_nullifiers(roots.nullifiers)?;
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
    fn test_persistent_tx_membership() {
        let (app_db, _dir) = open_test_db();
        let mut txs = app_db.open_transactions(EMPTY_HASH).unwrap();
        let tx_hash = test_hash(9);
        txs.insert(&Value::from(tx_hash)).unwrap();

        let roots = CanonicalRoots {
            transactions: txs.commitment(),
            ..CanonicalRoots::empty()
        };

        assert_eq!(
            app_db.tx_exists_batch(&roots, &[tx_hash]).unwrap(),
            vec![true]
        );
        let (present, _proof) = app_db.prove_tx(&roots, tx_hash).unwrap();
        assert!(present);
    }
}
