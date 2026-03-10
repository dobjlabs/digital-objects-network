use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use alloy::primitives::B256;
use anyhow::{Context, Result};
use hex::{FromHex, ToHex};
use pod2::middleware::Hash;
use rocksdb::{IteratorMode, Options, WriteBatch, DB};
use serde::{Deserialize, Serialize};

const SYNC_SLOT_KEY: &[u8] = b"sync:last_processed_slot";
const SYNC_BLOCK_KEY: &[u8] = b"sync:last_processed_block_number";
const TX_PREFIX: &[u8] = b"tx:";
const NULLIFIER_PREFIX: &[u8] = b"nullifier:";
const GSR_PREFIX: &[u8] = b"global_state_root:";
const SLOT_ROOT_PREFIX: &[u8] = b"slot_root:";

#[derive(Debug)]
pub struct DerivedState {
    pub transactions: HashSet<Hash>,
    pub nullifiers: HashSet<Hash>,
    pub global_state_roots: Vec<Hash>,
    pub gsr_block_numbers: HashMap<Hash, i64>,
}

#[derive(Debug, Clone, Copy)]
pub struct SyncProgress {
    pub last_processed_slot: u32,
    pub last_processed_block_number: Option<u32>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct SeenAt {
    slot: u32,
    block_number: u32,
}

pub struct Db {
    db: Arc<DB>,
}

impl Db {
    pub fn connect(db_path: &str) -> Result<Self> {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        let db = DB::open(&opts, db_path)
            .with_context(|| format!("Failed to open RocksDB at path {db_path}"))?;
        Ok(Self { db: Arc::new(db) })
    }

    pub fn load_state(&self) -> Result<DerivedState> {
        let mut transactions = HashSet::new();
        let mut nullifiers = HashSet::new();
        let mut gsr_entries: Vec<(u32, Hash)> = Vec::new();

        for entry in self.db.iterator(IteratorMode::Start) {
            let (key, value) = entry?;
            if key.starts_with(TX_PREFIX) {
                let hash_bytes = &key[TX_PREFIX.len()..];
                if let Ok(hash) = db_bytes_to_hash(hash_bytes) {
                    transactions.insert(hash);
                }
            } else if key.starts_with(NULLIFIER_PREFIX) {
                let hash_bytes = &key[NULLIFIER_PREFIX.len()..];
                if let Ok(hash) = db_bytes_to_hash(hash_bytes) {
                    nullifiers.insert(hash);
                }
            } else if key.starts_with(GSR_PREFIX) {
                if let (Some(block_number), Some((_slot, hash))) =
                    (gsr_key_block(&key), decode_gsr_value(&value))
                {
                    gsr_entries.push((block_number, hash));
                }
            }
        }

        // Sort GSR entries by block number to get ordered history
        gsr_entries.sort_by_key(|(block, _)| *block);
        let gsr_block_numbers = gsr_entries.iter().map(|&(b, h)| (h, b as i64)).collect();
        let global_state_roots = gsr_entries.into_iter().map(|(_, h)| h).collect();

        Ok(DerivedState {
            transactions,
            nullifiers,
            global_state_roots,
            gsr_block_numbers,
        })
    }

    pub fn last_processed_slot(&self) -> Result<Option<u32>> {
        Ok(self
            .last_progress()?
            .map(|progress| progress.last_processed_slot))
    }

    pub fn last_progress(&self) -> Result<Option<SyncProgress>> {
        let Some(slot_bytes) = self.db.get(SYNC_SLOT_KEY)? else {
            return Ok(None);
        };
        let slot = decode_u32(&slot_bytes).context("Invalid stored last_processed_slot")?;

        let block = match self.db.get(SYNC_BLOCK_KEY)? {
            Some(bytes) => {
                Some(decode_u32(&bytes).context("Invalid stored last_processed_block_number")?)
            }
            None => None,
        };

        Ok(Some(SyncProgress {
            last_processed_slot: slot,
            last_processed_block_number: block,
        }))
    }

    pub fn mark_slot_processed(&self, slot: u32, block_number: Option<u32>) -> Result<()> {
        let mut batch = WriteBatch::default();
        batch.put(SYNC_SLOT_KEY, slot.to_be_bytes());
        match block_number {
            Some(block) => batch.put(SYNC_BLOCK_KEY, block.to_be_bytes()),
            None => batch.delete(SYNC_BLOCK_KEY),
        }
        self.db.write(batch)?;
        Ok(())
    }

    pub fn persist_transaction(&self, hash: Hash, slot: u32, block_number: u32) -> Result<()> {
        let key = tx_key(hash);
        let value = serde_json::to_vec(&SeenAt { slot, block_number })?;
        self.db.put(key, value)?;
        Ok(())
    }

    pub fn persist_nullifier(&self, hash: Hash, slot: u32, block_number: u32) -> Result<()> {
        let key = nullifier_key(hash);
        let value = serde_json::to_vec(&SeenAt { slot, block_number })?;
        self.db.put(key, value)?;
        Ok(())
    }

    pub fn persist_global_state_root(
        &self,
        slot: u32,
        block_number: u32,
        hash: Hash,
    ) -> Result<()> {
        let key = gsr_key(block_number);
        let value = encode_gsr_value(slot, hash);
        self.db.put(key, value)?;
        Ok(())
    }

    pub fn set_slot_root(&self, slot: u32, root: Option<B256>) -> Result<()> {
        let key = slot_root_key(slot);
        match root {
            Some(root) => self.db.put(key, root.as_slice())?,
            None => self.db.delete(key)?,
        }
        Ok(())
    }

    pub fn slot_root(&self, slot: u32) -> Result<Option<B256>> {
        let Some(bytes) = self.db.get(slot_root_key(slot))? else {
            return Ok(None);
        };
        let raw: &[u8] = bytes.as_ref();
        let arr: [u8; 32] = raw
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid stored slot root bytes"))?;
        Ok(Some(B256::from(arr)))
    }

    pub fn rollback_to_slot(&self, keep_slot: Option<u32>) -> Result<()> {
        let mut batch = WriteBatch::default();

        // Remove any derived records written after the retained canonical slot.
        for entry in self.db.iterator(IteratorMode::Start) {
            let (key, value) = entry?;
            if key.starts_with(TX_PREFIX) || key.starts_with(NULLIFIER_PREFIX) {
                let should_delete = match keep_slot {
                    Some(keep) => match serde_json::from_slice::<SeenAt>(&value) {
                        Ok(seen_at) => seen_at.slot > keep,
                        Err(_) => true,
                    },
                    None => true,
                };
                if should_delete {
                    batch.delete(key);
                }
            } else if key.starts_with(GSR_PREFIX) {
                let should_delete = match keep_slot {
                    Some(keep) => match decode_gsr_value(&value) {
                        Some((slot, _hash)) => slot > keep,
                        None => true,
                    },
                    None => true,
                };
                if should_delete {
                    batch.delete(key);
                }
            } else if key.starts_with(SLOT_ROOT_PREFIX) {
                let should_delete = match keep_slot {
                    Some(keep) => slot_root_key_slot(&key).is_none_or(|slot| slot > keep),
                    None => true,
                };
                if should_delete {
                    batch.delete(key);
                }
            }
        }

        // Move sync cursor back to the retained slot (or reset completely).
        match keep_slot {
            Some(keep) => {
                batch.put(SYNC_SLOT_KEY, keep.to_be_bytes());
                batch.delete(SYNC_BLOCK_KEY);
            }
            None => {
                batch.delete(SYNC_SLOT_KEY);
                batch.delete(SYNC_BLOCK_KEY);
            }
        }

        self.db.write(batch)?;
        Ok(())
    }
}

fn tx_key(hash: Hash) -> Vec<u8> {
    [TX_PREFIX, &hash_to_db_bytes(hash)].concat()
}

fn nullifier_key(hash: Hash) -> Vec<u8> {
    [NULLIFIER_PREFIX, &hash_to_db_bytes(hash)].concat()
}

fn gsr_key(block_number: u32) -> Vec<u8> {
    [GSR_PREFIX, &block_number.to_be_bytes()].concat()
}

fn gsr_key_block(key: &[u8]) -> Option<u32> {
    let raw = key.strip_prefix(GSR_PREFIX)?;
    let arr: [u8; 4] = raw.try_into().ok()?;
    Some(u32::from_be_bytes(arr))
}

fn slot_root_key(slot: u32) -> Vec<u8> {
    [SLOT_ROOT_PREFIX, &slot.to_be_bytes()].concat()
}

fn slot_root_key_slot(key: &[u8]) -> Option<u32> {
    let raw = key.strip_prefix(SLOT_ROOT_PREFIX)?;
    let arr: [u8; 4] = raw.try_into().ok()?;
    Some(u32::from_be_bytes(arr))
}

fn encode_gsr_value(slot: u32, hash: Hash) -> Vec<u8> {
    let mut out = Vec::with_capacity(36);
    out.extend_from_slice(&slot.to_be_bytes());
    out.extend_from_slice(&hash_to_db_bytes(hash));
    out
}

fn decode_gsr_value(bytes: &[u8]) -> Option<(u32, Hash)> {
    if bytes.len() != 36 {
        return None;
    }
    let slot = u32::from_be_bytes(bytes[0..4].try_into().ok()?);
    let hash = db_bytes_to_hash(&bytes[4..36]).ok()?;
    Some((slot, hash))
}

/// Serialize a Hash to 32 raw bytes for RocksDB storage.
pub fn hash_to_db_bytes(hash: Hash) -> Vec<u8> {
    let hex_str: String = hash.encode_hex();
    hex::decode(hex_str).expect("ToHex output is always valid hex")
}

/// Deserialize a Hash from 32 raw bytes stored in RocksDB.
pub fn db_bytes_to_hash(bytes: &[u8]) -> Result<Hash> {
    Hash::from_hex(hex::encode(bytes)).context("Failed to deserialize Hash from db bytes")
}

/// Encode a Hash as a hex string for API responses.
pub fn hash_to_hex(hash: &Hash) -> String {
    hash.encode_hex()
}

fn decode_u32(bytes: &[u8]) -> Result<u32> {
    let arr: [u8; 4] = bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("expected exactly 4 bytes"))?;
    Ok(u32::from_be_bytes(arr))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::B256;
    use pod2::middleware::{hash_values, Value, EMPTY_HASH};
    use tempfile::TempDir;

    fn open_test_db() -> (Db, TempDir) {
        let dir = TempDir::new().expect("tempdir");
        let db = Db::connect(dir.path().to_str().unwrap()).expect("connect");
        (db, dir)
    }

    #[test]
    fn test_persist_and_load_transaction() {
        let (db, _dir) = open_test_db();
        let hash = EMPTY_HASH;
        db.persist_transaction(hash, 1, 100).unwrap();

        let state = db.load_state().unwrap();
        assert!(state.transactions.contains(&hash));
    }

    #[test]
    fn test_persist_and_load_nullifier() {
        let (db, _dir) = open_test_db();
        let hash = EMPTY_HASH;
        db.persist_nullifier(hash, 1, 100).unwrap();

        let state = db.load_state().unwrap();
        assert!(state.nullifiers.contains(&hash));
    }

    #[test]
    fn test_persist_and_load_global_state_roots_ordered() {
        let (db, _dir) = open_test_db();

        // Persist out of order to verify ordering
        let h0 = hash_values(&[Value::from(0)]);
        let h1 = hash_values(&[Value::from(1)]);
        let h2 = hash_values(&[Value::from(2)]);
        db.persist_global_state_root(10, 10, h0).unwrap();
        db.persist_global_state_root(5, 5, h1).unwrap();
        db.persist_global_state_root(20, 20, h2).unwrap();

        let state = db.load_state().unwrap();
        assert_eq!(state.global_state_roots, vec![h1, h0, h2]);
    }

    #[test]
    fn test_rollback_removes_gsrs_after_keep_slot() {
        let (db, _dir) = open_test_db();
        let h1 = hash_values(&[Value::from(1)]);
        let h2 = hash_values(&[Value::from(2)]);
        let h3 = hash_values(&[Value::from(3)]);
        db.persist_global_state_root(1, 1, h1).unwrap();
        db.persist_global_state_root(2, 2, h2).unwrap();
        db.persist_global_state_root(3, 3, h3).unwrap();

        db.rollback_to_slot(Some(1)).unwrap();

        let state = db.load_state().unwrap();
        assert_eq!(state.global_state_roots, vec![h1]);
    }

    #[test]
    fn test_rollback_to_slot_prunes_all_reorg_sensitive_state() {
        let (db, _dir) = open_test_db();
        let tx1 = hash_values(&[Value::from(11)]);
        let tx2 = hash_values(&[Value::from(12)]);
        let n1 = hash_values(&[Value::from(21)]);
        let n2 = hash_values(&[Value::from(22)]);
        let g1 = hash_values(&[Value::from(31)]);
        let g2 = hash_values(&[Value::from(32)]);
        let root1 = B256::from([1u8; 32]);
        let root2 = B256::from([2u8; 32]);

        db.persist_transaction(tx1, 1, 101).unwrap();
        db.persist_transaction(tx2, 2, 102).unwrap();
        db.persist_nullifier(n1, 1, 101).unwrap();
        db.persist_nullifier(n2, 2, 102).unwrap();
        db.persist_global_state_root(1, 101, g1).unwrap();
        db.persist_global_state_root(2, 102, g2).unwrap();
        db.set_slot_root(1, Some(root1)).unwrap();
        db.set_slot_root(2, Some(root2)).unwrap();
        db.mark_slot_processed(2, Some(102)).unwrap();

        db.rollback_to_slot(Some(1)).unwrap();

        let state = db.load_state().unwrap();
        assert!(state.transactions.contains(&tx1));
        assert!(!state.transactions.contains(&tx2));
        assert!(state.nullifiers.contains(&n1));
        assert!(!state.nullifiers.contains(&n2));
        assert_eq!(state.global_state_roots, vec![g1]);
        assert_eq!(db.slot_root(1).unwrap(), Some(root1));
        assert_eq!(db.slot_root(2).unwrap(), None);

        let progress = db.last_progress().unwrap().expect("progress exists");
        assert_eq!(progress.last_processed_slot, 1);
        assert_eq!(progress.last_processed_block_number, None);
    }

    #[test]
    fn test_rollback_to_none_resets_all_state() {
        let (db, _dir) = open_test_db();
        let tx = hash_values(&[Value::from(11)]);
        let n = hash_values(&[Value::from(21)]);
        let g = hash_values(&[Value::from(31)]);

        db.persist_transaction(tx, 1, 101).unwrap();
        db.persist_nullifier(n, 1, 101).unwrap();
        db.persist_global_state_root(1, 101, g).unwrap();
        db.set_slot_root(1, Some(B256::from([1u8; 32]))).unwrap();
        db.mark_slot_processed(1, Some(101)).unwrap();

        db.rollback_to_slot(None).unwrap();

        let state = db.load_state().unwrap();
        assert!(state.transactions.is_empty());
        assert!(state.nullifiers.is_empty());
        assert!(state.global_state_roots.is_empty());
        assert_eq!(db.slot_root(1).unwrap(), None);
        assert!(db.last_progress().unwrap().is_none());
    }
}
