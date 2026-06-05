use std::{collections::HashMap, fmt, sync::Arc};

use anyhow::{anyhow, Context, Result};
use pod2::{
    backends::plonky2::primitives::merkletree::{self, MerkleProof},
    middleware::{
        containers::{Array, Set},
        db::DB as PodDb,
        Hash, RawValue, Value, EMPTY_HASH,
    },
};
use rocksdb::{Options, TransactionDB, TransactionDBOptions};

use crate::head::StateRoots;

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

/// Whether the created `Array` holds `commitment` at `index`: the leaf there
/// must equal it. A prefetched index is only a hint until this confirms the
/// array actually holds the commitment at that position, so the read path and
/// the derivation collision check both call it to authenticate an index.
pub fn created_array_holds(created: &Array, index: i64, commitment: Hash) -> Result<bool> {
    Ok(created.get(index as usize)? == Some(Value::from(commitment)))
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

    pub fn open_next_state_history(&self, root: Hash) -> Result<Array> {
        Ok(Array::from_db(root, self.db_box())?)
    }

    /// Membership witness for each object commitment against the created `Array`
    /// at `roots.created`: `Some((array index, ArrayContains proof))` when the
    /// array authenticates the commitment at its prefetched index, `None` when
    /// absent (no index, or a leaf mismatch). The array is opened once for the
    /// whole batch.
    pub fn prove_created_for(
        &self,
        roots: &StateRoots,
        obj_commitments: &[Hash],
        indices: &HashMap<Hash, i64>,
    ) -> Result<Vec<Option<(i64, MerkleProof)>>> {
        let created = self.open_created(roots.created)?;
        obj_commitments
            .iter()
            .map(|commitment| match indices.get(commitment) {
                None => Ok(None),
                Some(&index) => match created.prove(index as usize) {
                    Ok((value, proof)) if value == Value::from(*commitment) => {
                        Ok(Some((index, proof)))
                    }
                    _ => Ok(None),
                },
            })
            .collect()
    }

    /// Membership bits for `obj_commitments` against the created `Array` at
    /// `roots.created`, using candidate indices prefetched from the Postgres
    /// created index. A commitment with no index is absent; otherwise the array
    /// leaf at its index must equal it -- the cross-check that authenticates the
    /// index against the authoritative root.
    pub fn created_exists_for(
        &self,
        roots: &StateRoots,
        obj_commitments: &[Hash],
        indices: &HashMap<Hash, i64>,
    ) -> Result<Vec<bool>> {
        let created = self.open_created(roots.created)?;
        obj_commitments
            .iter()
            .map(|commitment| match indices.get(commitment) {
                None => Ok(false),
                Some(&index) => created_array_holds(&created, index, *commitment),
            })
            .collect()
    }

    pub fn nullifier_exists_batch(
        &self,
        roots: &StateRoots,
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

pub use common::{db_bytes_to_hash, hash_to_db_bytes};

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
    fn test_created_membership_via_index() {
        let (app_db, _dir) = open_test_db();
        let mut created = app_db.open_created(EMPTY_HASH).unwrap();
        let obj_commitment = test_hash(9);
        created.insert(0, Value::from(obj_commitment)).unwrap();

        let roots = StateRoots {
            created: created.commitment(),
            ..StateRoots::empty()
        };
        let indices = HashMap::from([(obj_commitment, 0i64)]);

        assert_eq!(
            app_db
                .created_exists_for(&roots, &[obj_commitment], &indices)
                .unwrap(),
            vec![true]
        );
        let witnesses = app_db
            .prove_created_for(&roots, &[obj_commitment], &indices)
            .unwrap();
        let witness = witnesses.into_iter().next().unwrap();
        assert_eq!(witness.map(|(index, _proof)| index), Some(0));

        // A commitment with no index is absent, and an index the array root
        // does not actually contain resolves to absent too.
        let absent = test_hash(7);
        assert_eq!(
            app_db
                .created_exists_for(&roots, &[absent], &HashMap::new())
                .unwrap(),
            vec![false]
        );
        let empty_roots = StateRoots::empty();
        assert_eq!(
            app_db
                .created_exists_for(&empty_roots, &[obj_commitment], &indices)
                .unwrap(),
            vec![false]
        );
    }
}
