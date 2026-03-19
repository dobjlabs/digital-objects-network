use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, RwLock},
};

use anyhow::{anyhow, Context, Result};
use common::proof::BlobParser;
use pod2::{
    backends::plonky2::primitives::merkletree::MerkleProof,
    middleware::{containers::Array, containers::Set, Hash, Value},
};
use tracing::{info, warn};

use crate::{
    app_db::{AppDb, AppHead},
    sync_db::SlotJournal,
};
use txlib::StateRoot;

/// The maximum age of a GSR used as grounding for a transaction.
/// At one block per 12 seconds, this is one hour.
pub const MAX_GSR_AGE_BLOCKS: i64 = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotDelta {
    pub old_head: AppHead,
    pub new_head: AppHead,
}

#[derive(Debug, Clone, Copy)]
pub struct ApiStateSnapshot {
    pub head: AppHead,
}

#[derive(Debug, Clone)]
pub struct TxMembershipProof {
    pub tx_hash: Hash,
    pub present: bool,
    pub proof: MerkleProof,
}

#[derive(Debug, Clone)]
pub struct GroundingWitnessSnapshot {
    pub head: AppHead,
    pub source_tx_proofs: Vec<TxMembershipProof>,
}

struct InnerState {
    head: AppHead,
    recent_gsrs: HashMap<Hash, i64>,
}

struct WorkingState {
    head: AppHead,
    transactions: Set,
    nullifiers: Set,
    gsr_history: Array,
    recent_gsrs: HashMap<Hash, i64>,
}

/// Domain logic for the synchronizer: proof verification, state validation, and persistence.
///
/// `StateMachine` is intentionally decoupled from networking — it operates entirely on
/// raw byte slices and block numbers, making it straightforward to unit-test without a
/// live beacon node.
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
        let head = app_db.load_head()?;
        Ok(Self {
            state: RwLock::new(InnerState {
                head,
                recent_gsrs: HashMap::new(),
            }),
            app_db,
            proof_parser,
        })
    }

    pub fn reload_from_db(&self) -> Result<()> {
        let head = self.app_db.load_head()?;
        let mut state = self.write_state()?;
        state.head = head;
        Ok(())
    }

    pub fn replace_recent_gsrs(
        &self,
        recent_gsrs: impl IntoIterator<Item = (Hash, i64)>,
    ) -> Result<()> {
        let mut state = self.write_state()?;
        state.recent_gsrs = recent_gsrs.into_iter().collect();
        Ok(())
    }

    pub fn head_snapshot(&self) -> Result<AppHead> {
        Ok(self.read_state()?.head)
    }

    pub fn noop_delta(&self) -> Result<SlotDelta> {
        let head = self.head_snapshot()?;
        Ok(SlotDelta {
            old_head: head,
            new_head: head,
        })
    }

    fn snapshot_working_state(&self) -> Result<WorkingState> {
        let state = self.read_state()?;
        Ok(WorkingState {
            head: state.head,
            transactions: self
                .app_db
                .open_transactions(state.head.transactions_root)?,
            nullifiers: self.app_db.open_nullifiers(state.head.nullifiers_root)?,
            gsr_history: self.app_db.open_gsr_history(state.head.gsr_history_root)?,
            recent_gsrs: state.recent_gsrs.clone(),
        })
    }

    pub fn api_state_snapshot(&self) -> Result<ApiStateSnapshot> {
        Ok(ApiStateSnapshot {
            head: self.head_snapshot()?,
        })
    }

    pub fn tx_exists(&self, tx_hash: &Hash) -> Result<bool> {
        Ok(self.tx_exists_batch(std::slice::from_ref(tx_hash))?[0])
    }

    pub fn tx_exists_batch(&self, tx_hashes: &[Hash]) -> Result<Vec<bool>> {
        let head = self.head_snapshot()?;
        self.app_db.tx_exists_batch(&head, tx_hashes)
    }

    pub fn nullifier_exists_batch(&self, nullifiers: &[Hash]) -> Result<Vec<bool>> {
        let head = self.head_snapshot()?;
        self.app_db.nullifier_exists_batch(&head, nullifiers)
    }

    pub fn grounding_witness(&self, source_tx_hashes: &[Hash]) -> Result<GroundingWitnessSnapshot> {
        let head = self.head_snapshot()?;
        let source_tx_proofs = source_tx_hashes
            .iter()
            .map(|tx_hash| {
                let (present, proof) = self.app_db.prove_tx(&head, *tx_hash)?;
                Ok(TxMembershipProof {
                    tx_hash: *tx_hash,
                    present,
                    proof,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(GroundingWitnessSnapshot {
            head,
            source_tx_proofs,
        })
    }

    fn process_blob(
        &self,
        state: &mut WorkingState,
        bytes: &[u8],
        slot: u32,
        block_number: u32,
    ) -> Result<()> {
        let payload = match self.proof_parser.parse_blob(bytes) {
            Ok(Some(payload)) => payload,
            Ok(None) => {
                info!(
                    slot,
                    block_number, "Blob did not contain a valid TxFinalized proof; skipping"
                );
                return Ok(());
            }
            Err(err) => {
                warn!(
                    slot,
                    block_number,
                    ?err,
                    "Failed to parse/verify TxFinalized payload; skipping blob"
                );
                return Ok(());
            }
        };

        let Some(&gsr_block) = state.recent_gsrs.get(&payload.state_root_hash) else {
            warn!(
                slot,
                block_number,
                "Blob proof state_root_hash not found in recent GSR history; rejecting"
            );
            return Ok(());
        };
        let current_block = i64::from(block_number);
        let age = current_block - gsr_block;
        if age > MAX_GSR_AGE_BLOCKS {
            warn!(
                slot,
                block_number, gsr_block, age, "Blob proof state_root_hash is too old; rejecting"
            );
            return Ok(());
        }

        let tx_value = Value::from(payload.tx_final);
        if state.transactions.contains(&tx_value)? {
            warn!(slot, block_number, "Duplicate tx_final; rejecting");
            return Ok(());
        }

        let mut payload_nullifiers = HashSet::with_capacity(payload.nullifiers.len());
        for nullifier in &payload.nullifiers {
            if !payload_nullifiers.insert(*nullifier) {
                warn!(
                    slot,
                    block_number, "Duplicate nullifier within payload; rejecting"
                );
                return Ok(());
            }
            if state.nullifiers.contains(&Value::from(*nullifier))? {
                warn!(slot, block_number, "Duplicate nullifier; rejecting");
                return Ok(());
            }
        }

        state.transactions.insert(&tx_value)?;
        state.head.tx_count += 1;
        for nullifier in &payload.nullifiers {
            state.nullifiers.insert(&Value::from(*nullifier))?;
            state.head.nullifier_count += 1;
        }

        info!(
            slot,
            block_number,
            transaction_count = state.head.tx_count,
            nullifier_count = state.head.nullifier_count,
            "Validated blob state update in slot derivation"
        );
        Ok(())
    }

    pub fn derive_slot_delta(
        &self,
        slot: u32,
        block_number: u32,
        blob_payloads: &[(u32, Vec<u8>)],
    ) -> Result<SlotDelta> {
        let mut working = self.snapshot_working_state()?;
        let old_head = working.head;

        for (blob_index, bytes) in blob_payloads {
            self.process_blob(&mut working, bytes, slot, block_number)
                .with_context(|| {
                    format!(
                        "Failed to process blob at slot {}, blob_index {}",
                        slot, blob_index
                    )
                })?;
        }

        let prior_gsrs_root = old_head.gsr_history_root;
        let new_gsr = StateRoot::from_roots(
            i64::from(block_number),
            working.transactions.commitment(),
            working.nullifiers.commitment(),
            prior_gsrs_root,
        )
        .hash();

        working
            .gsr_history
            .insert(old_head.gsr_count as usize, Value::from(new_gsr))?;
        working.recent_gsrs.insert(new_gsr, i64::from(block_number));

        let min_block = i64::from(block_number) - MAX_GSR_AGE_BLOCKS;
        working
            .recent_gsrs
            .retain(|_, seen_block| *seen_block >= min_block);

        let new_head = AppHead {
            transactions_root: working.transactions.commitment(),
            nullifiers_root: working.nullifiers.commitment(),
            state_root_gsrs_root: prior_gsrs_root,
            gsr_history_root: working.gsr_history.commitment(),
            current_gsr: Some(new_gsr),
            current_block_number: Some(block_number),
            tx_count: working.head.tx_count,
            nullifier_count: working.head.nullifier_count,
            gsr_count: old_head.gsr_count + 1,
        };

        info!(
            slot,
            block_number,
            gsr_count = new_head.gsr_count,
            "Slot data"
        );

        Ok(SlotDelta { old_head, new_head })
    }

    pub fn apply_delta_to_memory(&self, delta: &SlotDelta) -> Result<()> {
        let mut state = self.write_state()?;
        state.head = delta.new_head;
        if let (Some(current_gsr), Some(current_block_number)) = (
            delta.new_head.current_gsr,
            delta.new_head.current_block_number,
        ) {
            state
                .recent_gsrs
                .insert(current_gsr, i64::from(current_block_number));
            let min_block = i64::from(current_block_number) - MAX_GSR_AGE_BLOCKS;
            state
                .recent_gsrs
                .retain(|_, seen_block| *seen_block >= min_block);
        }
        Ok(())
    }

    pub fn apply_delta_to_db(&self, delta: &SlotDelta) -> Result<()> {
        self.app_db.store_head(&delta.new_head)
    }

    pub fn apply_journal(&self, journal: &SlotJournal) -> Result<()> {
        self.app_db.store_head(&journal.new_head)
    }

    pub fn rollback_journals(&self, journals: &[SlotJournal]) -> Result<()> {
        if let Some(final_journal) = journals.last() {
            self.app_db.store_head(&final_journal.old_head)?;
        }
        self.reload_from_db()
    }

    pub fn log_current_state(&self) -> Result<()> {
        let head = self.head_snapshot()?;
        info!(
            transaction_count = head.tx_count,
            nullifier_count = head.nullifier_count,
            gsr_count = head.gsr_count,
            current_gsr = ?head.current_gsr,
            "Current state"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_db::AppDb;
    use common::proof::MockBlobParser;
    use pod2::middleware::hash_values;
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
            .map(|h| format!("\"{:#}\"", h))
            .collect::<Vec<_>>()
            .join(",");
        format!(
            r#"{{"tx_final":"{:#}","nullifiers":[{}],"state_root_hash":"{:#}"}}"#,
            tx_final, nullifiers_json, state_root
        )
        .into_bytes()
    }

    fn seed_gsr0(sm: &StateMachine) -> Hash {
        let d = sm.derive_slot_delta(0, 0, &[]).unwrap();
        sm.apply_delta_to_db(&d).unwrap();
        sm.apply_delta_to_memory(&d).unwrap();
        sm.head_snapshot().unwrap().current_gsr.unwrap()
    }

    #[test]
    fn test_persistent_head_roundtrip() {
        let (sm, _dir) = make_sm();
        let delta = sm.derive_slot_delta(1, 7, &[]).unwrap();
        sm.apply_delta_to_db(&delta).unwrap();
        sm.reload_from_db().unwrap();
        let head = sm.head_snapshot().unwrap();
        assert_eq!(head.current_block_number, Some(7));
        assert_eq!(head.gsr_count, 1);
    }

    #[test]
    fn test_accepts_valid_blob_and_updates_counts() {
        let (sm, _dir) = make_sm();
        let gsr0 = seed_gsr0(&sm);
        sm.replace_recent_gsrs([(gsr0, 0)]).unwrap();

        let tx_final = unique_hash(10);
        let nullifier = unique_hash(11);
        let blob = mock_txn_bytes(tx_final, &[nullifier], gsr0);
        let delta = sm.derive_slot_delta(1, 1, &[(0, blob)]).unwrap();
        sm.apply_delta_to_db(&delta).unwrap();
        sm.apply_delta_to_memory(&delta).unwrap();

        let head = sm.head_snapshot().unwrap();
        assert_eq!(head.tx_count, 1);
        assert_eq!(head.nullifier_count, 1);
        assert_eq!(head.gsr_count, 2);
        assert!(sm.tx_exists(&tx_final).unwrap());
        assert_eq!(sm.nullifier_exists_batch(&[nullifier]).unwrap(), vec![true]);
    }

    #[test]
    fn test_rejects_unknown_grounding_gsr() {
        let (sm, _dir) = make_sm();
        seed_gsr0(&sm);
        sm.replace_recent_gsrs([]).unwrap();

        let tx_final = unique_hash(21);
        let blob = mock_txn_bytes(tx_final, &[], unique_hash(99));
        let delta = sm.derive_slot_delta(2, 2, &[(0, blob)]).unwrap();
        assert_eq!(delta.new_head.tx_count, delta.old_head.tx_count);
    }

    #[test]
    fn test_rollbacks_restore_previous_head() {
        let (sm, _dir) = make_sm();
        let d0 = sm.derive_slot_delta(0, 0, &[]).unwrap();
        sm.apply_delta_to_db(&d0).unwrap();
        sm.apply_delta_to_memory(&d0).unwrap();
        let h0 = sm.head_snapshot().unwrap();

        let d1 = sm.derive_slot_delta(1, 1, &[]).unwrap();
        sm.apply_delta_to_db(&d1).unwrap();
        sm.apply_delta_to_memory(&d1).unwrap();

        sm.rollback_journals(&[SlotJournal {
            slot: 1,
            old_head: h0,
            new_head: d1.new_head,
        }])
        .unwrap();
        assert_eq!(sm.head_snapshot().unwrap(), h0);
    }
}
