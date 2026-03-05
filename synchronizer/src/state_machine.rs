use std::{
    collections::HashSet,
    sync::{Arc, RwLock},
};

use anyhow::{anyhow, Result};
use pod2::middleware::Hash;
use tracing::{info, warn};

use txlib::StateRoot;

use crate::app_db::{AppDb, DerivedState};
use crate::proof::BlobParser;
use crate::sync_db::SlotJournal;

#[derive(Debug, Clone, Default)]
pub struct SlotDelta {
    pub tx_hashes: Vec<Hash>,
    pub nullifiers: Vec<Hash>,
    pub gsr_block_numbers: Vec<u32>,
    pub gsr_hashes: Vec<Hash>,
}

struct InnerState {
    transactions: HashSet<Hash>,
    nullifiers: HashSet<Hash>,
    global_state_roots: Vec<Hash>,
}

pub struct StateMachine {
    app_db: AppDb,
    state: RwLock<InnerState>,
    proof_parser: Arc<dyn BlobParser>,
}

impl StateMachine {
    fn read_state(&self) -> Result<std::sync::RwLockReadGuard<'_, InnerState>> {
        self.state
            .read()
            .map_err(|e| anyhow!("state read lock poisoned: {e}"))
    }

    fn write_state(&self) -> Result<std::sync::RwLockWriteGuard<'_, InnerState>> {
        self.state
            .write()
            .map_err(|e| anyhow!("state write lock poisoned: {e}"))
    }

    pub fn new(app_db: AppDb, proof_parser: Arc<dyn BlobParser>) -> Result<Self> {
        let DerivedState {
            transactions,
            nullifiers,
            global_state_roots,
        } = app_db.load_state()?;
        Ok(Self {
            state: RwLock::new(InnerState {
                transactions,
                nullifiers,
                global_state_roots,
            }),
            app_db,
            proof_parser,
        })
    }

    pub fn reload_from_db(&self) -> Result<()> {
        let DerivedState {
            transactions,
            nullifiers,
            global_state_roots,
        } = self.app_db.load_state()?;
        let mut state = self.write_state()?;
        state.transactions = transactions;
        state.nullifiers = nullifiers;
        state.global_state_roots = global_state_roots;
        Ok(())
    }

    pub fn process_blob(
        &self,
        bytes: &[u8],
        slot: u32,
        block_number: Option<u32>,
        delta: &mut SlotDelta,
    ) -> Result<()> {
        let Some(payload) = self.proof_parser.parse_blob(bytes)? else {
            info!(
                slot,
                block_number, "Blob did not contain a valid TxFinalized proof; skipping"
            );
            return Ok(());
        };

        {
            let state = self.read_state()?;
            if !state.global_state_roots.contains(&payload.state_root_hash) {
                warn!(
                    slot,
                    block_number,
                    "Blob proof state_root_hash not found in known GSR history; rejecting"
                );
                return Ok(());
            }
        }

        {
            let mut state = self.write_state()?;

            if !state.transactions.insert(payload.tx_final) {
                warn!(slot, block_number, "Duplicate tx_final; rejecting");
                return Ok(());
            }

            for nullifier in &payload.nullifiers {
                if state.nullifiers.contains(nullifier) {
                    warn!(slot, block_number, "Duplicate nullifier; rejecting");
                    state.transactions.remove(&payload.tx_final);
                    return Ok(());
                }
            }

            delta.tx_hashes.push(payload.tx_final);
            for nullifier in &payload.nullifiers {
                state.nullifiers.insert(*nullifier);
                delta.nullifiers.push(*nullifier);
            }

            info!(
                slot,
                block_number,
                transaction_count = state.transactions.len(),
                nullifier_count = state.nullifiers.len(),
                "Applied blob state update in-memory"
            );
        }

        Ok(())
    }

    pub fn advance_block(&self, slot: u32, block_number: u32, delta: &mut SlotDelta) -> Result<()> {
        let mut state = self.write_state()?;

        let new_gsr = StateRoot::new(
            block_number as i64,
            &state.transactions,
            &state.nullifiers,
            &state.global_state_roots,
        )
        .hash();

        state.global_state_roots.push(new_gsr);
        delta.gsr_block_numbers.push(block_number);
        delta.gsr_hashes.push(new_gsr);

        info!(
            slot,
            block_number,
            gsr_count = state.global_state_roots.len(),
            "Computed new GSR in-memory"
        );
        Ok(())
    }

    pub fn apply_slot_delta(
        &self,
        slot: u32,
        block_number: Option<u32>,
        delta: &SlotDelta,
    ) -> Result<()> {
        self.app_db.apply_slot_delta(
            slot,
            block_number,
            &delta.tx_hashes,
            &delta.nullifiers,
            &delta.gsr_block_numbers,
            &delta.gsr_hashes,
        )
    }

    pub fn apply_journal(&self, journal: &SlotJournal, block_number: Option<u32>) -> Result<()> {
        self.app_db.apply_slot_delta(
            journal.slot,
            block_number,
            &journal.tx_hashes,
            &journal.nullifiers,
            &journal.gsr_block_numbers,
            &journal.gsr_hashes,
        )
    }

    pub fn rollback_journals(&self, journals: &[SlotJournal]) -> Result<()> {
        for journal in journals {
            for tx in &journal.tx_hashes {
                self.app_db.delete_transaction(*tx)?;
            }
            for nullifier in &journal.nullifiers {
                self.app_db.delete_nullifier(*nullifier)?;
            }
            for block_number in &journal.gsr_block_numbers {
                self.app_db.delete_global_state_root(*block_number)?;
            }
        }
        self.reload_from_db()
    }

    pub fn state_snapshot(&self) -> Result<(Vec<Hash>, Vec<Hash>, Vec<Hash>)> {
        let state = self.read_state()?;
        Ok((
            state.transactions.iter().copied().collect(),
            state.nullifiers.iter().copied().collect(),
            state.global_state_roots.clone(),
        ))
    }

    pub fn log_current_state(&self) -> Result<()> {
        let state = self.read_state()?;
        info!(
            transaction_count = state.transactions.len(),
            nullifier_count = state.nullifiers.len(),
            gsr_count = state.global_state_roots.len(),
            "Current in-memory state snapshot"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proof::MockBlobParser;
    use hex::ToHex;
    use pod2::middleware::{hash_values, Value};
    use tempfile::TempDir;

    fn make_sm() -> (StateMachine, TempDir) {
        let dir = TempDir::new().unwrap();
        let app_db = AppDb::connect(dir.path().to_str().unwrap()).unwrap();
        let sm = StateMachine::new(app_db, Arc::new(MockBlobParser)).unwrap();
        (sm, dir)
    }

    fn unique_hash(n: i64) -> Hash {
        hash_values(&[Value::from(n)])
    }

    fn mock_txn_bytes(tx_final: Hash, nullifiers: &[Hash], state_root: Hash) -> Vec<u8> {
        let nullifiers_json = nullifiers
            .iter()
            .map(|h| format!("\"{}\"", h.encode_hex::<String>()))
            .collect::<Vec<_>>()
            .join(",");
        format!(
            r#"{{"tx_final":"{}","nullifiers":[{}],"state_root_hash":"{}"}}"#,
            tx_final.encode_hex::<String>(),
            nullifiers_json,
            state_root.encode_hex::<String>()
        )
        .into_bytes()
    }

    #[test]
    fn test_happy_path_single_tx() {
        let (sm, _dir) = make_sm();

        let mut seed = SlotDelta::default();
        sm.advance_block(0, 0, &mut seed).unwrap();
        sm.apply_slot_delta(0, Some(0), &seed).unwrap();

        let gsr0 = sm.state_snapshot().unwrap().2[0];

        let mut d1 = SlotDelta::default();
        let tx_hash = unique_hash(1);
        let nullifier = unique_hash(2);
        sm.process_blob(
            &mock_txn_bytes(tx_hash, &[nullifier], gsr0),
            1,
            Some(1),
            &mut d1,
        )
        .unwrap();
        sm.advance_block(1, 1, &mut d1).unwrap();
        sm.apply_slot_delta(1, Some(1), &d1).unwrap();

        let (txns, nullifiers, _) = sm.state_snapshot().unwrap();
        assert!(txns.contains(&tx_hash));
        assert!(nullifiers.contains(&nullifier));
    }
}
