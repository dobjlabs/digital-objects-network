use std::{collections::HashSet, sync::Arc};

use anyhow::{Context, Result};
use rocksdb::{IteratorMode, Options, WriteBatch, DB};
use serde::{Deserialize, Serialize};

const SYNC_SLOT_KEY: &[u8] = b"sync:last_processed_slot";
const SYNC_BLOCK_KEY: &[u8] = b"sync:last_processed_block_number";
const TX_PREFIX: &[u8] = b"tx:";
const NULLIFIER_PREFIX: &[u8] = b"nullifier:";

#[derive(Debug)]
pub struct DerivedState {
    pub transactions: HashSet<String>,
    pub nullifiers: HashSet<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct SyncProgress {
    pub last_processed_slot: u32,
    pub last_processed_block_number: Option<u32>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct SeenAt {
    slot: u32,
    block_number: Option<u32>,
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

    pub fn init(&self) -> Result<()> {
        Ok(())
    }

    pub fn load_state(&self) -> Result<DerivedState> {
        let mut transactions = HashSet::new();
        let mut nullifiers = HashSet::new();

        for entry in self.db.iterator(IteratorMode::Start) {
            let (key, _value) = entry?;
            if key.starts_with(TX_PREFIX) {
                if let Ok(id) = String::from_utf8(key[TX_PREFIX.len()..].to_vec()) {
                    transactions.insert(id);
                }
            } else if key.starts_with(NULLIFIER_PREFIX) {
                if let Ok(id) = String::from_utf8(key[NULLIFIER_PREFIX.len()..].to_vec()) {
                    nullifiers.insert(id);
                }
            }
        }

        Ok(DerivedState {
            transactions,
            nullifiers,
        })
    }

    pub fn last_processed_slot(&self) -> Result<Option<u32>> {
        Ok(self.last_progress()?.map(|progress| progress.last_processed_slot))
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

    pub fn persist_transaction(
        &self,
        object_id: &str,
        slot: u32,
        block_number: Option<u32>,
    ) -> Result<()> {
        let key = tx_key(object_id);
        let value = serde_json::to_vec(&SeenAt { slot, block_number })?;
        self.db.put(key, value)?;
        Ok(())
    }

    pub fn persist_nullifier(
        &self,
        object_id: &str,
        slot: u32,
        block_number: Option<u32>,
    ) -> Result<()> {
        let key = nullifier_key(object_id);
        let value = serde_json::to_vec(&SeenAt { slot, block_number })?;
        self.db.put(key, value)?;
        Ok(())
    }
}

fn tx_key(id: &str) -> Vec<u8> {
    [TX_PREFIX, id.as_bytes()].concat()
}

fn nullifier_key(id: &str) -> Vec<u8> {
    [NULLIFIER_PREFIX, id.as_bytes()].concat()
}

fn decode_u32(bytes: &[u8]) -> Result<u32> {
    let arr: [u8; 4] = bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("expected exactly 4 bytes"))?;
    Ok(u32::from_be_bytes(arr))
}
