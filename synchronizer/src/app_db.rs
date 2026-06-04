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

fn created_index_key(commitment: Hash) -> Vec<u8> {
    let mut k = Vec::with_capacity(35);
    k.extend_from_slice(b"ci/");
    k.extend_from_slice(&hash_to_db_bytes(commitment));
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

    pub fn open_created(&self, root: Hash) -> Result<Array> {
        Ok(Array::from_db(root, self.db_box())?)
    }

    pub fn open_nullifiers(&self, root: Hash) -> Result<Set> {
        Ok(Set::from_db(root, self.db_box())?)
    }

    pub fn open_gsr_history(&self, root: Hash) -> Result<Array> {
        Ok(Array::from_db(root, self.db_box())?)
    }

    /// Record an object commitment's position in the created `Array`.
    ///
    /// The cache is a plain `commitment -> index` map: not Merkleized, not part
    /// of any committed root. It is only ever read as a hint -- every membership
    /// or proof query cross-checks the index against the `Array` at the queried
    /// root, so a stale entry (e.g. one left behind by an abandoned reorg branch)
    /// resolves to "absent" rather than a wrong answer.
    pub fn created_index_put(&self, commitment: Hash, index: i64) -> Result<()> {
        self.db
            .put(created_index_key(commitment), index.to_le_bytes())
            .map_err(|err| anyhow!("rocksdb created-index put failed: {err}"))
    }

    pub fn created_index_get(&self, commitment: Hash) -> Result<Option<i64>> {
        match self.db.get(created_index_key(commitment))? {
            None => Ok(None),
            Some(bytes) => {
                let limbs: [u8; 8] = bytes
                    .as_slice()
                    .try_into()
                    .map_err(|_| anyhow!("invalid created-index entry length"))?;
                Ok(Some(i64::from_le_bytes(limbs)))
            }
        }
    }

    /// `(index, membership proof)` for one object commitment against the created
    /// `Array` at `roots.created`, or `(false, None)` when it is not present
    /// there. The cache supplies the candidate index; proving it against the
    /// array is what authenticates membership (and rejects stale cache hits).
    pub fn prove_created(
        &self,
        roots: &CanonicalRoots,
        obj_commitment: Hash,
    ) -> Result<(bool, Option<(i64, MerkleProof)>)> {
        let Some(index) = self.created_index_get(obj_commitment)? else {
            return Ok((false, None));
        };
        let created = self.open_created(roots.created)?;
        match created.prove(index as usize) {
            Ok((value, proof)) if value == Value::from(obj_commitment) => {
                Ok((true, Some((index, proof))))
            }
            _ => Ok((false, None)),
        }
    }

    pub fn created_exists_batch(
        &self,
        roots: &CanonicalRoots,
        obj_commitments: &[Hash],
    ) -> Result<Vec<bool>> {
        let created = self.open_created(roots.created)?;
        obj_commitments
            .iter()
            .map(|commitment| match self.created_index_get(*commitment)? {
                None => Ok(false),
                Some(index) => Ok(created.get(index as usize)? == Some(Value::from(*commitment))),
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
    fn test_persistent_created_membership() {
        let (app_db, _dir) = open_test_db();
        let mut created = app_db.open_created(EMPTY_HASH).unwrap();
        let obj_commitment = test_hash(9);
        created.insert(0, Value::from(obj_commitment)).unwrap();
        app_db.created_index_put(obj_commitment, 0).unwrap();

        let roots = CanonicalRoots {
            created: created.commitment(),
            ..CanonicalRoots::empty()
        };

        assert_eq!(
            app_db
                .created_exists_batch(&roots, &[obj_commitment])
                .unwrap(),
            vec![true]
        );
        let (present, witness) = app_db.prove_created(&roots, obj_commitment).unwrap();
        assert!(present);
        assert_eq!(witness.map(|(index, _proof)| index), Some(0));

        // A commitment never recorded in the cache is absent, and a cache hit
        // that the array root does not actually contain resolves to absent too.
        let absent = test_hash(7);
        assert_eq!(
            app_db.created_exists_batch(&roots, &[absent]).unwrap(),
            vec![false]
        );
        let empty_roots = CanonicalRoots::empty();
        assert_eq!(
            app_db
                .created_exists_batch(&empty_roots, &[obj_commitment])
                .unwrap(),
            vec![false]
        );
    }
}
