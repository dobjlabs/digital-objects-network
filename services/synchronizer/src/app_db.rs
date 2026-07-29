use std::{collections::HashMap, fmt, sync::Arc};

use anyhow::{anyhow, Context, Result};
use pod2::{
    backends::plonky2::primitives::merkletree::{self, MerkleProof},
    middleware::{
        containers::{Array, ContainerKind, Set},
        db::{Read, DB as PodDb, TX as PodTx},
        Hash, RawValue, Value, EMPTY_HASH,
    },
};
use rocksdb::{DBAccess, Options, ReadOptions, Transaction, TransactionDB, TransactionDBOptions};

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

fn kind_key(root: Hash) -> Vec<u8> {
    let mut k = Vec::with_capacity(2 + 4);
    k.extend_from_slice(b"k/");
    k.extend_from_slice(&RawValue(root.0).to_bytes());
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

fn load_node_db(db: &impl DBAccess, hash: Hash) -> Result<Option<merkletree::Node>> {
    if hash == EMPTY_HASH {
        return Ok(Some(merkletree::Node::Intermediate(
            merkletree::Intermediate::new(EMPTY_HASH, EMPTY_HASH),
        )));
    }

    let node_key = node_key(hash);
    match db
        .get_opt(&node_key, &ReadOptions::default())
        .map_err(|e| anyhow!("rocksdb: get failed: {e}"))?
    {
        None => Ok(None),
        Some(bytes) => Ok(Some(merkletree::Node::decode(bytes.as_ref())?)),
    }
}

fn store_node_tx<'a>(tx: &Transaction<'a, TransactionDB>, node: merkletree::Node) -> Result<()> {
    let node_key = node_key(node.hash());
    tx.put(&node_key, node.encode()?)
        .map_err(|e| anyhow!("rocksdb transaction put failed: {e}"))
}

impl merkletree::db::Read for AppDb {
    fn load_node(&self, hash: Hash) -> Result<Option<merkletree::Node>> {
        load_node_db(&*self.db, hash)
    }
}

impl merkletree::db::DB for AppDb {
    fn tx<'a>(&'a self) -> Box<dyn merkletree::db::TX + 'a> {
        PodDb::tx(self)
    }
}

pub(crate) struct AppTx<'a> {
    tx: rocksdb::Transaction<'a, rocksdb::TransactionDB>,
    db: AppDb,
}

impl<'a> merkletree::db::Read for AppTx<'a> {
    fn load_node(&self, hash: Hash) -> anyhow::Result<Option<merkletree::Node>> {
        load_node_db(&self.tx, hash)
    }
}

impl<'a> merkletree::db::TX for AppTx<'a> {
    fn store_node(&mut self, node: merkletree::Node) -> anyhow::Result<()> {
        store_node_tx(&self.tx, node)
    }
    fn commit(self: Box<Self>) -> anyhow::Result<()> {
        panic!("use middleware::db::TX::commit")
    }
}

impl<'a> Read for AppTx<'a> {
    fn load_value(&self, raw: RawValue) -> anyhow::Result<Option<Value>> {
        match self.tx.get(value_key(raw))? {
            None => Ok(None),
            Some(bytes) => Ok(Some({
                if bytes.is_empty() {
                    Value::from(raw)
                } else {
                    Value::from_bytes(bytes.as_ref(), self.db.clone_box())?
                }
            })),
        }
    }
    fn load_kind(&self, root: Hash) -> anyhow::Result<Option<ContainerKind>> {
        if root == EMPTY_HASH {
            return Ok(Some(
                *ContainerKind::default()
                    .set_dictionary()
                    .set_set()
                    .set_array(),
            ));
        }
        // We use `get_for_update` because this method is part of a transaction, and it will be
        // used by `update_kind`, so we want and exclusive lock after the value is read to
        // guarantee no data-races in the merge update.
        self.tx
            .get_for_update(kind_key(root), true)
            .map(|opt| {
                opt.map(|bytes| match bytes.len() {
                    1 => Ok(ContainerKind(bytes[0])),
                    l => Err(anyhow!("db: invalid kind len: {}", l)),
                })
            })?
            .transpose()
    }
}

impl<'a> PodTx for AppTx<'a> {
    fn store_value(&mut self, value: Value) -> anyhow::Result<()> {
        let value_key = value_key(value.raw());
        if let Some(old_value_bytes) = self.tx.get(&value_key)? {
            // Never overwrite an old value with a RawValue.  Skip overwrite if old value is
            // already non-RawValue.
            if value.is_raw() || !old_value_bytes.is_empty() {
                return Ok(());
            }
        };
        let value_bytes = if value.is_raw() {
            // For RawValue we store an empty vector because it's a duplicate of the key.
            // This way we can easily check for RawValue without decoding.
            vec![]
        } else {
            Value::to_bytes(&value)
        };
        Ok(self.tx.put(value_key, value_bytes)?)
    }
    fn update_kind(&mut self, root: Hash, kind: ContainerKind) -> anyhow::Result<()> {
        let kind = match self.load_kind(root).expect("ok") {
            Some(old_kind) => ContainerKind(old_kind.0 | kind.0),
            None => kind,
        };
        let kind_key = kind_key(root);
        Ok(self.tx.put(&kind_key, [kind.0])?)
    }
    fn commit(self: Box<Self>) -> anyhow::Result<()> {
        Ok(self.tx.commit()?)
    }
}

impl Read for AppDb {
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
    fn load_kind(&self, root: Hash) -> anyhow::Result<Option<ContainerKind>> {
        if root == EMPTY_HASH {
            return Ok(Some(
                *ContainerKind::default()
                    .set_dictionary()
                    .set_set()
                    .set_array(),
            ));
        }
        Ok(self.db.get(kind_key(root)).map(|opt| {
            opt.map(|bytes| {
                assert_eq!(1, bytes.len());
                ContainerKind(bytes[0])
            })
        })?)
    }
}

impl PodDb for AppDb {
    fn tx<'a>(&'a self) -> Box<dyn PodTx + 'a> {
        Box::new(AppTx {
            tx: self.db.transaction(),
            db: self.clone(),
        })
    }
    fn clone_box(&self) -> Box<dyn PodDb> {
        Box::new(self.clone())
    }
}

pub use payload::{db_bytes_to_hash, hash_to_db_bytes};

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
