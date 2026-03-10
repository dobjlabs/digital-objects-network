use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use anyhow::{Context, Result};
use hex::{FromHex, ToHex};
use pod2::middleware::Hash;
use rocksdb::{IteratorMode, Options, WriteBatch, DB};

const TX_PREFIX: &[u8] = b"tx:";
const NULLIFIER_PREFIX: &[u8] = b"nullifier:";
const GSR_PREFIX: &[u8] = b"global_state_root:";

#[derive(Debug)]
pub struct DerivedState {
    pub transactions: HashSet<Hash>,
    pub nullifiers: HashSet<Hash>,
    pub global_state_roots: Vec<Hash>,
    pub gsr_block_numbers: HashMap<Hash, i64>,
}

const PRESENT_VALUE: &[u8] = &[1];

pub struct AppDb {
    db: Arc<DB>,
}

impl AppDb {
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
                if let (Some(block_number), Some(hash)) =
                    (gsr_key_block(&key), decode_gsr_value(&value))
                {
                    gsr_entries.push((block_number, hash));
                }
            }
        }

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

    pub fn delete_slot_delta(
        &self,
        tx_hashes: &[Hash],
        nullifiers: &[Hash],
        gsr_block_numbers: &[u32],
    ) -> Result<()> {
        let mut batch = WriteBatch::default();
        for tx in tx_hashes {
            batch.delete(tx_key(*tx));
        }
        for nullifier in nullifiers {
            batch.delete(nullifier_key(*nullifier));
        }
        for block_number in gsr_block_numbers {
            batch.delete(gsr_key(*block_number));
        }
        self.db.write(batch)?;
        Ok(())
    }

    pub fn apply_delta(
        &self,
        tx_hashes: &[Hash],
        nullifiers: &[Hash],
        gsr_block_numbers: &[u32],
        gsr_hashes: &[Hash],
    ) -> Result<()> {
        let mut batch = WriteBatch::default();

        for tx in tx_hashes {
            batch.put(tx_key(*tx), PRESENT_VALUE);
        }

        for nullifier in nullifiers {
            batch.put(nullifier_key(*nullifier), PRESENT_VALUE);
        }

        for (block_number, gsr) in gsr_block_numbers.iter().zip(gsr_hashes.iter()) {
            batch.put(gsr_key(*block_number), encode_gsr_value(*gsr));
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

fn encode_gsr_value(hash: Hash) -> Vec<u8> {
    hash_to_db_bytes(hash)
}

fn decode_gsr_value(bytes: &[u8]) -> Option<Hash> {
    if bytes.len() != 32 {
        return None;
    }
    db_bytes_to_hash(bytes).ok()
}

pub fn hash_to_db_bytes(hash: Hash) -> Vec<u8> {
    let hex_str: String = hash.encode_hex();
    hex::decode(hex_str).expect("ToHex output is always valid hex")
}

pub fn db_bytes_to_hash(bytes: &[u8]) -> Result<Hash> {
    Hash::from_hex(hex::encode(bytes)).context("Failed to deserialize Hash from db bytes")
}

#[cfg(test)]
mod tests {
    use super::*;
    use pod2::middleware::{hash_values, Value, EMPTY_HASH};
    use tempfile::TempDir;

    fn open_test_db() -> (AppDb, TempDir) {
        let dir = TempDir::new().expect("tempdir");
        let app_db = AppDb::connect(dir.path().to_str().unwrap()).expect("connect");
        (app_db, dir)
    }

    #[test]
    fn test_persist_and_load_transaction() {
        let (app_db, _dir) = open_test_db();
        let hash = EMPTY_HASH;
        app_db.apply_delta(&[hash], &[], &[], &[]).unwrap();

        let state = app_db.load_state().unwrap();
        assert!(state.transactions.contains(&hash));
    }

    #[test]
    fn test_persist_and_load_nullifier() {
        let (app_db, _dir) = open_test_db();
        let hash = EMPTY_HASH;
        app_db.apply_delta(&[], &[hash], &[], &[]).unwrap();

        let state = app_db.load_state().unwrap();
        assert!(state.nullifiers.contains(&hash));
    }

    #[test]
    fn test_persist_and_load_global_state_roots_ordered() {
        let (app_db, _dir) = open_test_db();

        let h0 = hash_values(&[Value::from(0)]);
        let h1 = hash_values(&[Value::from(1)]);
        let h2 = hash_values(&[Value::from(2)]);

        app_db.apply_delta(&[], &[], &[10], &[h0]).unwrap();
        app_db.apply_delta(&[], &[], &[5], &[h1]).unwrap();
        app_db.apply_delta(&[], &[], &[20], &[h2]).unwrap();

        let state = app_db.load_state().unwrap();
        assert_eq!(state.global_state_roots, vec![h1, h0, h2]);
    }

    #[test]
    fn test_delete_slot_delta_removes_all_slot_keys() {
        let (app_db, _dir) = open_test_db();
        let tx = hash_values(&[Value::from(10)]);
        let nullifier = hash_values(&[Value::from(11)]);
        let gsr = hash_values(&[Value::from(12)]);

        app_db
            .apply_delta(&[tx], &[nullifier], &[700], &[gsr])
            .unwrap();
        let before = app_db.load_state().unwrap();
        assert!(before.transactions.contains(&tx));
        assert!(before.nullifiers.contains(&nullifier));
        assert!(before.global_state_roots.contains(&gsr));

        app_db
            .delete_slot_delta(&[tx], &[nullifier], &[700])
            .unwrap();
        let after = app_db.load_state().unwrap();
        assert!(!after.transactions.contains(&tx));
        assert!(!after.nullifiers.contains(&nullifier));
        assert!(!after.global_state_roots.contains(&gsr));
    }
}
