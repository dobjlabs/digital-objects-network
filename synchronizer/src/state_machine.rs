use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, RwLock},
};

use alloy::primitives::B256;
use anyhow::{anyhow, Result};
use pod2::middleware::Hash;
use tracing::{info, warn};

/// The maximum age of a GSR used as grounding for a transaction.
/// At one block per 12 seconds, this is one hour.
const MAX_GSR_AGE_BLOCKS: i64 = 300;

use txlib::StateRoot;

use crate::db::{Db, DerivedState, SyncProgress};
use crate::proof::BlobParser;

/// In-memory view of the consensus state, kept in sync with the database.
struct InnerState {
    /// Set of accepted transaction hashes; used for duplicate detection.
    transactions: HashSet<Hash>,
    /// Set of spent nullifiers; a nullifier appearing twice indicates a double-spend.
    nullifiers: HashSet<Hash>,
    /// Ordered history of Global State Roots, one per processed block.
    /// Blobs may reference any GSR in this history, not just the latest.
    global_state_roots: Vec<Hash>,
    /// Maps each known GSR hash to the EL block number at which it was produced.
    /// Used to enforce the maximum GSR age limit on incoming blobs.
    gsr_block_numbers: HashMap<Hash, i64>,
}

/// Domain logic for the synchronizer: proof verification, state validation, and persistence.
///
/// `StateMachine` is intentionally decoupled from networking — it operates entirely on
/// raw byte slices and block numbers, making it straightforward to unit-test without a
/// live beacon node.
pub struct StateMachine {
    db: Db,
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

    /// Restore in-memory state from the database and return a ready `StateMachine`.
    pub fn new(db: Db, proof_parser: Arc<dyn BlobParser>) -> Result<Self> {
        let DerivedState {
            transactions,
            nullifiers,
            global_state_roots,
            gsr_block_numbers,
        } = db.load_state()?;
        Ok(Self {
            state: RwLock::new(InnerState {
                transactions,
                nullifiers,
                global_state_roots,
                gsr_block_numbers,
            }),
            db,
            proof_parser,
        })
    }

    /// Process raw blob content (post-blob-encoding extraction).
    ///
    /// Steps:
    /// 1. Attempt to parse and cryptographically verify the blob as a `TxnFinalized` payload.
    ///    Blobs that don't match our format are silently skipped (they may belong to other apps).
    /// 2. Reject payloads whose `state_root_hash` is not in our GSR history.
    ///    This ensures every transaction is grounded in a state root we have computed ourselves.
    /// 3. Check for duplicate `tx_final` and spent nullifiers before writing anything,
    ///    so the update is all-or-nothing: either all nullifiers are accepted or none are.
    pub fn process_blob(&self, bytes: &[u8], slot: u32, block_number: Option<u32>) -> Result<()> {
        let Some(payload) = self.proof_parser.parse_blob(bytes)? else {
            info!(
                slot,
                block_number, "Blob did not contain a valid TxFinalized proof; skipping"
            );
            return Ok(());
        };

        // A payload is only valid if it references a GSR we have previously computed,
        // and that GSR must be no more than MAX_GSR_AGE_BLOCKS old.
        {
            let state = self.read_state()?;
            let Some(&gsr_block) = state.gsr_block_numbers.get(&payload.state_root_hash) else {
                warn!(
                    slot,
                    block_number,
                    "Blob proof state_root_hash not found in known GSR history; rejecting"
                );
                return Ok(());
            };
            if let Some(current_block) = block_number {
                let age = current_block as i64 - gsr_block;
                if age > MAX_GSR_AGE_BLOCKS {
                    warn!(
                        slot,
                        block_number,
                        gsr_block,
                        age,
                        "Blob proof state_root_hash is too old; rejecting"
                    );
                    return Ok(());
                }
            }
        }

        // All uniqueness checks and DB writes happen under the write lock so that
        // concurrent calls cannot interleave partial state.
        //
        // Strategy: insert tx_final optimistically, then scan all nullifiers.
        // On any collision, roll back the tx_final insertion and bail without touching the DB.
        // Only after all checks pass do we write nullifiers to the DB.
        {
            let mut state = self.write_state()?;

            if !state.transactions.insert(payload.tx_final) {
                warn!(slot, block_number, "Duplicate tx_final; rejecting");
                return Ok(());
            }

            for nullifier in &payload.nullifiers {
                if state.nullifiers.contains(nullifier) {
                    warn!(slot, block_number, "Duplicate nullifier; rejecting");
                    // Roll back the optimistic tx_final insertion.
                    state.transactions.remove(&payload.tx_final);
                    return Ok(());
                }
            }

            self.db
                .persist_transaction(payload.tx_final, slot, block_number)?;
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
    ///
    /// Must be called exactly once per block, **after** all blobs for that block have been
    /// processed. The new GSR commits to the accumulated set of transaction commitments and
    /// nullifiers so far, so subsequent provers can reference it as their `state_root_hash`.
    pub fn advance_block(&self, slot: u32, block_number: u32) -> Result<()> {
        let mut state = self.write_state()?;

        let new_gsr = StateRoot::new(
            block_number as i64,
            &state.transactions,
            &state.nullifiers,
            &state.global_state_roots,
        )
        .hash();
        state.global_state_roots.push(new_gsr);
        self.db
            .persist_global_state_root(slot, block_number, new_gsr)?;

        info!(
            slot,
            block_number,
            gsr_count = state.global_state_roots.len(),
            "Computed and persisted new GSR"
        );
        Ok(())
    }

    /// Returns `(transactions, nullifiers, global_state_roots)` as owned vecs.
    /// Primarily used in tests; callers that need only one field should add a dedicated accessor.
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

    pub fn last_processed_slot(&self) -> Result<Option<u32>> {
        self.db.last_processed_slot()
    }

    pub fn last_progress(&self) -> Result<Option<SyncProgress>> {
        self.db.last_progress()
    }

    pub fn mark_slot_processed(&self, slot: u32, block_number: Option<u32>) -> Result<()> {
        self.db.mark_slot_processed(slot, block_number)
    }

    pub fn set_slot_root(&self, slot: u32, root: Option<B256>) -> Result<()> {
        self.db.set_slot_root(slot, root)
    }

    pub fn slot_root(&self, slot: u32) -> Result<Option<B256>> {
        self.db.slot_root(slot)
    }

    pub fn rollback_to_slot(&self, keep_slot: Option<u32>) -> Result<()> {
        self.db.rollback_to_slot(keep_slot)?;
        let DerivedState {
            transactions,
            nullifiers,
            global_state_roots,
        } = self.db.load_state()?;
        let mut state = self.write_state()?;
        state.transactions = transactions;
        state.nullifiers = nullifiers;
        state.global_state_roots = global_state_roots;
        Ok(())
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
        sm.advance_block(0, 0).unwrap();
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
        sm.advance_block(0, 0).unwrap();
        let gsr0 = sm.state_snapshot().unwrap().2[0];

        let tx1 = unique_hash(1);
        let null1 = unique_hash(2);
        sm.process_blob(&mock_txn_bytes(tx1, &[null1], gsr0), 1, Some(1))
            .unwrap();
        sm.advance_block(1, 1).unwrap();

        let gsr1 = sm.state_snapshot().unwrap().2[1];
        assert_ne!(gsr0, gsr1);

        let tx2 = unique_hash(3);
        let null2 = unique_hash(4);
        sm.process_blob(&mock_txn_bytes(tx2, &[null2], gsr1), 2, Some(2))
            .unwrap();
        sm.advance_block(2, 2).unwrap();

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
        sm.advance_block(0, 0).unwrap();
        let gsr0 = sm.state_snapshot().unwrap().2[0];

        let tx1 = unique_hash(1);
        sm.process_blob(&mock_txn_bytes(tx1, &[], gsr0), 1, Some(1))
            .unwrap();
        sm.advance_block(1, 1).unwrap();

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
        sm.advance_block(0, 0).unwrap();
        let gsr0 = sm.state_snapshot().unwrap().2[0];

        let tx_final = unique_hash(1);
        let bytes = mock_txn_bytes(tx_final, &[], gsr0);

        sm.process_blob(&bytes, 1, Some(1)).unwrap();
        sm.process_blob(&bytes, 1, Some(1)).unwrap(); // duplicate; silently rejected

        let (txns, _, _) = sm.state_snapshot().unwrap();
        assert_eq!(txns.len(), 1);
    }

    #[test]
    fn test_duplicate_nullifier_rejected() {
        let (sm, _dir) = make_sm();
        sm.advance_block(0, 0).unwrap();
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
        sm.advance_block(0, 0).unwrap();
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
        sm.advance_block(0, 0).unwrap();

        let bogus_gsr = unique_hash(999);
        let tx_final = unique_hash(1);
        sm.process_blob(&mock_txn_bytes(tx_final, &[], bogus_gsr), 1, Some(1))
            .unwrap();

        let (txns, _, _) = sm.state_snapshot().unwrap();
        assert!(txns.is_empty());
    }

    #[test]
    fn test_stale_gsr_rejected() {
        let (sm, _dir) = make_sm();
        sm.advance_block(0).unwrap();
        let gsr0 = sm.state_snapshot().unwrap().2[0];

        // Advance 301 more blocks so gsr0 is 301 blocks old when the blob arrives.
        for i in 1..=301 {
            sm.advance_block(i).unwrap();
        }

        let tx = unique_hash(1);
        sm.process_blob(&mock_txn_bytes(tx, &[], gsr0), 0, Some(301))
            .unwrap();

        let (txns, _, _) = sm.state_snapshot().unwrap();
        assert!(txns.is_empty());
    }

    #[test]
    fn test_gsr_at_limit_accepted() {
        let (sm, _dir) = make_sm();
        sm.advance_block(0).unwrap();
        let gsr0 = sm.state_snapshot().unwrap().2[0];

        // Advance 300 more blocks so gsr0 is exactly 300 blocks old — at the limit.
        for i in 1..=300 {
            sm.advance_block(i).unwrap();
        }

        let tx = unique_hash(1);
        sm.process_blob(&mock_txn_bytes(tx, &[], gsr0), 0, Some(300))
            .unwrap();

        let (txns, _, _) = sm.state_snapshot().unwrap();
        assert!(txns.contains(&tx));
    }

    #[test]
    fn test_invalid_blob_skipped() {
        let (sm, _dir) = make_sm();
        sm.advance_block(0, 0).unwrap();

        sm.process_blob(b"not json", 1, Some(1)).unwrap();

        let (txns, nullifiers, _) = sm.state_snapshot().unwrap();
        assert!(txns.is_empty());
        assert!(nullifiers.is_empty());
    }

    #[test]
    fn test_rollback_reloads_gsrs_from_retained_slot() {
        let (sm, _dir) = make_sm();
        sm.advance_block(0, 0).unwrap();
        sm.advance_block(1, 1).unwrap();
        sm.advance_block(2, 2).unwrap();
        assert_eq!(sm.state_snapshot().unwrap().2.len(), 3);

        sm.rollback_to_slot(Some(1)).unwrap();

        let (_, _, gsrs) = sm.state_snapshot().unwrap();
        assert_eq!(gsrs.len(), 2);
    }

    #[test]
    fn test_reorg_rollback_restores_in_memory_sets() {
        let (sm, _dir) = make_sm();
        sm.advance_block(0, 0).unwrap();
        let gsr0 = sm.state_snapshot().unwrap().2[0];

        let tx1 = unique_hash(101);
        let n1 = unique_hash(201);
        sm.process_blob(&mock_txn_bytes(tx1, &[n1], gsr0), 1, Some(1))
            .unwrap();
        sm.advance_block(1, 1).unwrap();
        let gsr1 = sm.state_snapshot().unwrap().2[1];

        let tx2 = unique_hash(102);
        let n2 = unique_hash(202);
        sm.process_blob(&mock_txn_bytes(tx2, &[n2], gsr1), 2, Some(2))
            .unwrap();
        sm.advance_block(2, 2).unwrap();

        sm.rollback_to_slot(Some(1)).unwrap();

        let (txns, nullifiers, gsrs) = sm.state_snapshot().unwrap();
        assert!(txns.contains(&tx1));
        assert!(!txns.contains(&tx2));
        assert!(nullifiers.contains(&n1));
        assert!(!nullifiers.contains(&n2));
        assert_eq!(gsrs.len(), 2);
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
            frontend::MultiPodBuilder,
            middleware::Params,
        };
        use pod2utils::macros::BuildContext;
        use std::collections::HashSet;
        use txlib::{Object, TxBuilder};

        let params = Params::default();
        let vd_set = &*DEFAULT_VD_SET;
        let shrunk_main_pod_build = ShrunkMainPodSetup::new(&params).build().unwrap();

        // Set up state machine with real ProofParser.
        let dir = TempDir::new().unwrap();
        let db = Db::connect(dir.path().to_str().unwrap()).unwrap();
        let sm =
            StateMachine::new(db, Arc::new(crate::proof::ProofParser::new().unwrap())).unwrap();

        // Seed GSR0 (empty state).
        sm.advance_block(0, 0).unwrap();
        let gsr0 = sm.state_snapshot().unwrap().2[0];

        // Build a txlib StateRoot matching the empty GSR0 and verify it agrees.
        let state_root = Arc::new(StateRoot {
            block_number: 0,
            transactions: pod2::middleware::containers::Set::new(HashSet::new()),
            nullifiers: pod2::middleware::containers::Set::new(HashSet::new()),
            gsrs: pod2::middleware::containers::Array::new(vec![]),
        });
        assert_eq!(
            state_root.hash(),
            gsr0,
            "txlib StateRoot must match computed GSR0"
        );

        // Prove a transaction using txlib's TxBuilder.
        let txlib_modules = vec![Arc::new(txlib::predicates::module())];
        let mut builder = MultiPodBuilder::new(&params, vd_set);
        let mut ctx = BuildContext {
            builder: &mut builder,
            modules: &txlib_modules,
        };

        let obj = Object::new(std::collections::HashMap::new());
        let mut tx_builder = TxBuilder::new(&mut ctx, &[], state_root);
        tx_builder.insert(&mut ctx, obj);
        let (st_finalized, tx) = tx_builder.finalize(&mut ctx);
        ctx.builder.reveal(&st_finalized).unwrap();

        let solution = builder.solve().unwrap();
        let pod = solution.prove(&Prover {}).unwrap().pods.pop().unwrap();
        pod.pod.verify().unwrap();

        let compressed_proof = shrink_compress_pod(&shrunk_main_pod_build, pod).unwrap();

        // tx.dict().commitment() is the public tx_final value committed to in the proof.
        let tx_final = tx.dict().commitment();
        let nullifiers: Vec<Hash> = tx
            .nullifiers
            .set()
            .iter()
            .map(|v| Hash(v.raw().0))
            .collect();

        let payload = Payload {
            proof: PayloadProof::Plonky2(Box::new(compressed_proof)),
            tx_final,
            state_root_hash: gsr0,
            nullifiers: nullifiers.clone(),
        };
        sm.process_blob(&payload.to_bytes(), 1, Some(1)).unwrap();

        let (txns, spent_nullifiers, _) = sm.state_snapshot().unwrap();
        assert!(txns.contains(&tx_final));
        for n in &nullifiers {
            assert!(spent_nullifiers.contains(n));
        }
    }
}
