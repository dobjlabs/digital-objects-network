use std::{
    collections::HashSet,
    sync::{Arc, RwLock},
};

use anyhow::{anyhow, Result};
use pod2::middleware::Hash;
use tracing::{info, warn};

use crate::db::{Db, DerivedState, SyncProgress};
use crate::gsr::compute_global_state_root;
use crate::proof::BlobParser;

struct InnerState {
    transactions: HashSet<Hash>,
    nullifiers: HashSet<Hash>,
    global_state_roots: Vec<Hash>,
}

pub struct StateMachine {
    db: Db,
    state: RwLock<InnerState>,
    proof_parser: Arc<dyn BlobParser>,
}

impl StateMachine {
    pub fn new(db: Db, proof_parser: Arc<dyn BlobParser>) -> Result<Self> {
        let DerivedState {
            transactions,
            nullifiers,
            global_state_roots,
        } = db.load_state()?;
        Ok(Self {
            state: RwLock::new(InnerState {
                transactions,
                nullifiers,
                global_state_roots,
            }),
            db,
            proof_parser,
        })
    }

    /// Process raw blob content (post-blob-encoding extraction).
    /// Parses the proof, validates it against known state, and updates state on success.
    pub fn process_blob(&self, bytes: &[u8], slot: u32, block_number: Option<u32>) -> Result<()> {
        let Some(payload) = self.proof_parser.parse_blob(bytes)? else {
            info!(
                slot,
                block_number, "Blob did not contain a valid TxnFinalized proof; skipping"
            );
            return Ok(());
        };

        // Validate that the payload's state_root_hash is in our known history.
        {
            let state = self
                .state
                .read()
                .map_err(|e| anyhow!("state read lock poisoned: {e}"))?;
            if !state.global_state_roots.contains(&payload.state_root_hash) {
                warn!(
                    slot,
                    block_number,
                    "Blob proof state_root_hash not found in known GSR history; rejecting"
                );
                return Ok(());
            }
        }

        // Apply state updates atomically: check uniqueness before writing anything.
        {
            let mut state = self
                .state
                .write()
                .map_err(|e| anyhow!("state write lock poisoned: {e}"))?;

            if !state.transactions.insert(payload.tx_hash) {
                warn!(slot, block_number, "Duplicate tx_hash; rejecting");
                return Ok(());
            }

            for nullifier in &payload.nullifiers {
                if state.nullifiers.contains(nullifier) {
                    warn!(slot, block_number, "Duplicate nullifier; rejecting");
                    state.transactions.remove(&payload.tx_hash);
                    return Ok(());
                }
            }

            self.db
                .persist_transaction(payload.tx_hash, slot, block_number)?;
            for nullifier in &payload.nullifiers {
                state.nullifiers.insert(*nullifier);
                self.db.persist_nullifier(*nullifier, slot, block_number)?;
            }

            info!(
                slot,
                block_number,
                transaction_count = state.transactions.len(),
                nullifier_count = state.nullifiers.len(),
                "Applied blob state update"
            );
        }

        Ok(())
    }

    /// Compute and persist a new GSR for the given block, appending it to the history.
    /// Must be called once per processed block, after all blobs for that block have been applied.
    pub fn advance_block(&self, block_number: i64) -> Result<()> {
        let mut state = self
            .state
            .write()
            .map_err(|e| anyhow!("state write lock poisoned: {e}"))?;

        let new_gsr = compute_global_state_root(
            &state.transactions,
            &state.nullifiers,
            &state.global_state_roots,
            block_number,
        );
        state.global_state_roots.push(new_gsr);
        self.db.persist_global_state_root(block_number, new_gsr)?;

        info!(
            block_number,
            gsr_count = state.global_state_roots.len(),
            "Computed and persisted new GSR"
        );
        Ok(())
    }

    pub fn state_snapshot(&self) -> Result<(Vec<Hash>, Vec<Hash>, Vec<Hash>)> {
        let state = self
            .state
            .read()
            .map_err(|e| anyhow!("state read lock poisoned: {e}"))?;
        Ok((
            state.transactions.iter().copied().collect(),
            state.nullifiers.iter().copied().collect(),
            state.global_state_roots.clone(),
        ))
    }

    pub fn log_current_state(&self) -> Result<()> {
        let state = self
            .state
            .read()
            .map_err(|e| anyhow!("state read lock poisoned: {e}"))?;
        info!(
            transaction_count = state.transactions.len(),
            nullifier_count = state.nullifiers.len(),
            gsr_count = state.global_state_roots.len(),
            "Current in-memory state snapshot"
        );
        Ok(())
    }

    pub fn last_processed_slot(&self) -> Result<Option<u32>> {
        self.db.last_processed_slot()
    }

    pub fn last_progress(&self) -> Result<Option<SyncProgress>> {
        self.db.last_progress()
    }

    pub fn mark_slot_processed(&self, slot: u32, block_number: Option<u32>) -> Result<()> {
        self.db.mark_slot_processed(slot, block_number)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::proof::MockBlobParser;
    use hex::ToHex;
    use pod2::middleware::{hash_values, Value};
    use tempfile::TempDir;

    fn make_sm() -> (StateMachine, TempDir) {
        let dir = TempDir::new().unwrap();
        let db = Db::connect(dir.path().to_str().unwrap()).unwrap();
        let sm = StateMachine::new(db, Arc::new(MockBlobParser)).unwrap();
        (sm, dir)
    }

    fn unique_hash(n: i64) -> Hash {
        hash_values(&[Value::from(n)])
    }

    fn mock_txn_bytes(tx_hash: Hash, nullifiers: &[Hash], state_root: Hash) -> Vec<u8> {
        let nullifiers_json = nullifiers
            .iter()
            .map(|h| format!("\"{}\"", h.encode_hex::<String>()))
            .collect::<Vec<_>>()
            .join(",");
        format!(
            r#"{{"tx_hash":"{}","nullifiers":[{}],"state_root_hash":"{}"}}"#,
            tx_hash.encode_hex::<String>(),
            nullifiers_json,
            state_root.encode_hex::<String>()
        )
        .into_bytes()
    }

    #[test]
    fn test_happy_path_single_tx() {
        let (sm, _dir) = make_sm();
        sm.advance_block(0).unwrap();
        let gsr0 = sm.state_snapshot().unwrap().2[0];

        let tx_hash = unique_hash(1);
        let nullifier = unique_hash(2);
        sm.process_blob(&mock_txn_bytes(tx_hash, &[nullifier], gsr0), 1, Some(1))
            .unwrap();

        let (txns, nullifiers, _) = sm.state_snapshot().unwrap();
        assert!(txns.contains(&tx_hash));
        assert!(nullifiers.contains(&nullifier));
    }

    #[test]
    fn test_sequence_across_blocks() {
        let (sm, _dir) = make_sm();
        sm.advance_block(0).unwrap();
        let gsr0 = sm.state_snapshot().unwrap().2[0];

        let tx1 = unique_hash(1);
        let null1 = unique_hash(2);
        sm.process_blob(&mock_txn_bytes(tx1, &[null1], gsr0), 1, Some(1))
            .unwrap();
        sm.advance_block(1).unwrap();

        let gsr1 = sm.state_snapshot().unwrap().2[1];
        assert_ne!(gsr0, gsr1);

        let tx2 = unique_hash(3);
        let null2 = unique_hash(4);
        sm.process_blob(&mock_txn_bytes(tx2, &[null2], gsr1), 2, Some(2))
            .unwrap();
        sm.advance_block(2).unwrap();

        let (txns, nullifiers, gsrs) = sm.state_snapshot().unwrap();
        assert!(txns.contains(&tx1));
        assert!(txns.contains(&tx2));
        assert!(nullifiers.contains(&null1));
        assert!(nullifiers.contains(&null2));
        assert_eq!(gsrs.len(), 3);
    }

    #[test]
    fn test_old_gsr_still_valid() {
        let (sm, _dir) = make_sm();
        sm.advance_block(0).unwrap();
        let gsr0 = sm.state_snapshot().unwrap().2[0];

        let tx1 = unique_hash(1);
        sm.process_blob(&mock_txn_bytes(tx1, &[], gsr0), 1, Some(1))
            .unwrap();
        sm.advance_block(1).unwrap();

        // tx2 is grounded against gsr0, not the newer gsr1 — still valid
        let tx2 = unique_hash(2);
        sm.process_blob(&mock_txn_bytes(tx2, &[], gsr0), 1, Some(1))
            .unwrap();

        let (txns, _, _) = sm.state_snapshot().unwrap();
        assert!(txns.contains(&tx1));
        assert!(txns.contains(&tx2));
    }

    #[test]
    fn test_duplicate_tx_rejected() {
        let (sm, _dir) = make_sm();
        sm.advance_block(0).unwrap();
        let gsr0 = sm.state_snapshot().unwrap().2[0];

        let tx_hash = unique_hash(1);
        let bytes = mock_txn_bytes(tx_hash, &[], gsr0);

        sm.process_blob(&bytes, 1, Some(1)).unwrap();
        sm.process_blob(&bytes, 1, Some(1)).unwrap(); // duplicate; silently rejected

        let (txns, _, _) = sm.state_snapshot().unwrap();
        assert_eq!(txns.len(), 1);
    }

    #[test]
    fn test_duplicate_nullifier_rejected() {
        let (sm, _dir) = make_sm();
        sm.advance_block(0).unwrap();
        let gsr0 = sm.state_snapshot().unwrap().2[0];

        let null = unique_hash(10);

        let tx1 = unique_hash(1);
        sm.process_blob(&mock_txn_bytes(tx1, &[null], gsr0), 1, Some(1))
            .unwrap();

        let tx2 = unique_hash(2);
        sm.process_blob(&mock_txn_bytes(tx2, &[null], gsr0), 1, Some(1))
            .unwrap(); // rejected: null already spent

        let (txns, nullifiers, _) = sm.state_snapshot().unwrap();
        assert!(txns.contains(&tx1));
        assert!(!txns.contains(&tx2));
        assert_eq!(nullifiers.len(), 1);
    }

    #[test]
    fn test_nullifier_collision_is_atomic() {
        let (sm, _dir) = make_sm();
        sm.advance_block(0).unwrap();
        let gsr0 = sm.state_snapshot().unwrap().2[0];

        let spent = unique_hash(10);
        let fresh_a = unique_hash(11);
        let fresh_b = unique_hash(12);

        let tx1 = unique_hash(1);
        sm.process_blob(&mock_txn_bytes(tx1, &[spent], gsr0), 1, Some(1))
            .unwrap();

        // tx2 has [fresh_a, spent, fresh_b] — 'spent' is a duplicate
        let tx2 = unique_hash(2);
        sm.process_blob(
            &mock_txn_bytes(tx2, &[fresh_a, spent, fresh_b], gsr0),
            1,
            Some(1),
        )
        .unwrap(); // rejected in full

        let (txns, nullifiers, _) = sm.state_snapshot().unwrap();
        assert!(!txns.contains(&tx2));
        assert!(!nullifiers.contains(&fresh_a));
        assert!(!nullifiers.contains(&fresh_b));
    }

    #[test]
    fn test_unknown_gsr_rejected() {
        let (sm, _dir) = make_sm();
        sm.advance_block(0).unwrap();

        let bogus_gsr = unique_hash(999);
        let tx_hash = unique_hash(1);
        sm.process_blob(&mock_txn_bytes(tx_hash, &[], bogus_gsr), 1, Some(1))
            .unwrap();

        let (txns, _, _) = sm.state_snapshot().unwrap();
        assert!(txns.is_empty());
    }

    #[test]
    fn test_invalid_blob_skipped() {
        let (sm, _dir) = make_sm();
        sm.advance_block(0).unwrap();

        sm.process_blob(b"not json", 1, Some(1)).unwrap();

        let (txns, nullifiers, _) = sm.state_snapshot().unwrap();
        assert!(txns.is_empty());
        assert!(nullifiers.is_empty());
    }

    #[test]
    #[ignore = "slow: requires Plonky2 proving (builds circuit on first run, cached thereafter)"]
    fn test_e2e_real_proof() {
        use common::{
            payload::{Payload, PayloadProof},
            shrink::{shrink_compress_pod, ShrunkMainPodSetup},
        };
        use pod2::{
            backends::plonky2::{basetypes::DEFAULT_VD_SET, mainpod::Prover},
            frontend::{MainPodBuilder, Operation},
            middleware::{containers::Set, Params, Value},
        };
        use std::collections::HashSet;

        let params = Params::default();
        let vd_set = &*DEFAULT_VD_SET;
        let shrunk_main_pod_build = ShrunkMainPodSetup::new(&params).build().unwrap();

        let dir = TempDir::new().unwrap();
        let db = Db::connect(dir.path().to_str().unwrap()).unwrap();
        let sm =
            StateMachine::new(db, Arc::new(crate::proof::ProofParser::new().unwrap())).unwrap();

        sm.advance_block(0).unwrap();
        let gsr0 = sm.state_snapshot().unwrap().2[0];

        let tx_hash = unique_hash(42);
        let null1 = unique_hash(43);
        let null2 = unique_hash(44);

        let module = pod2::lang::load_module(
            crate::proof::TXN_FINALIZED_PREDICATE,
            "txn_finalized",
            &params,
            &[],
        )
        .unwrap();
        let pred = module.predicate_ref_by_name("TxnFinalized").unwrap();

        let nullifiers_set = Value::from(Set::new(
            [null1, null2]
                .iter()
                .map(|h| Value::from(*h))
                .collect::<HashSet<_>>(),
        ));

        let mut builder = MainPodBuilder::new(&params, vd_set);
        let st0 = builder.priv_op(Operation::eq(1, 1)).unwrap();
        builder
            .op(
                true,
                vec![
                    (0, Value::from(tx_hash)),
                    (1, nullifiers_set),
                    (2, Value::from(gsr0)),
                ],
                Operation::custom(pred, [st0]),
            )
            .unwrap();

        let pod = builder.prove(&Prover {}).unwrap();
        pod.pod.verify().unwrap();
        let compressed_proof = shrink_compress_pod(&shrunk_main_pod_build, pod).unwrap();

        let payload = Payload {
            proof: PayloadProof::Plonky2(Box::new(compressed_proof)),
            tx_hash,
            state_root_hash: gsr0,
            nullifiers: vec![null1, null2],
        };
        sm.process_blob(&payload.to_bytes(), 1, Some(1)).unwrap();

        let (txns, nullifiers, _) = sm.state_snapshot().unwrap();
        assert!(txns.contains(&tx_hash));
        assert!(nullifiers.contains(&null1));
        assert!(nullifiers.contains(&null2));
    }
}
