use std::{collections::HashSet, sync::Arc};

use alloy::primitives::B256;
use anyhow::{Context, Result};
use rocksdb::{IteratorMode, Options, WriteBatch, DB};
use serde::{Deserialize, Serialize};

const SYNC_SLOT_KEY: &[u8] = b"sync:last_processed_slot";
const SYNC_BLOCK_KEY: &[u8] = b"sync:last_processed_block_number";
const SLOT_ROOT_PREFIX: &[u8] = b"slot_root:";
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

fn tx_key(id: &str) -> Vec<u8> {
    [TX_PREFIX, id.as_bytes()].concat()
}

fn nullifier_key(id: &str) -> Vec<u8> {
    [NULLIFIER_PREFIX, id.as_bytes()].concat()
}

fn slot_root_key(slot: u32) -> Vec<u8> {
    [SLOT_ROOT_PREFIX, &slot.to_be_bytes()].concat()
}

fn slot_root_key_slot(key: &[u8]) -> Option<u32> {
    let raw = key.strip_prefix(SLOT_ROOT_PREFIX)?;
    let arr: [u8; 4] = raw.try_into().ok()?;
    Some(u32::from_be_bytes(arr))
}

fn decode_u32(bytes: &[u8]) -> Result<u32> {
    let arr: [u8; 4] = bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("expected exactly 4 bytes"))?;
    Ok(u32::from_be_bytes(arr))
}
