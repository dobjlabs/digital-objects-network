use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, RwLock},
};

use anyhow::{anyhow, Context, Result};
use pod2::middleware::Hash;
use tracing::{info, warn};

/// The maximum age of a GSR used as grounding for a transaction.
/// At one block per 12 seconds, this is one hour.
const MAX_GSR_AGE_BLOCKS: i64 = 300;

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

#[derive(Clone)]
struct WorkingState {
    transactions: HashSet<Hash>,
    nullifiers: HashSet<Hash>,
    global_state_roots: Vec<Hash>,
    gsr_block_numbers: HashMap<Hash, i64>,
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
        let DerivedState {
            transactions,
            nullifiers,
            global_state_roots,
            gsr_block_numbers,
        } = app_db.load_state()?;
        Ok(Self {
            state: RwLock::new(InnerState {
                transactions,
                nullifiers,
                global_state_roots,
                gsr_block_numbers,
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
            gsr_block_numbers,
        } = self.app_db.load_state()?;
        let mut state = self.write_state()?;
        state.transactions = transactions;
        state.nullifiers = nullifiers;
        state.global_state_roots = global_state_roots;
        state.gsr_block_numbers = gsr_block_numbers;
        Ok(())
    }

    fn snapshot_working_state(&self) -> Result<WorkingState> {
        let state = self.read_state()?;
        Ok(WorkingState {
            transactions: state.transactions.clone(),
            nullifiers: state.nullifiers.clone(),
            global_state_roots: state.global_state_roots.clone(),
            gsr_block_numbers: state.gsr_block_numbers.clone(),
        })
    }

    /// Process raw blob content (post-blob-encoding extraction).
    ///
    /// Steps:
    /// 1. Attempt to parse and cryptographically verify the blob as a `TxnFinalized` payload.
    ///    Blobs that don't match our format are silently skipped (they may belong to other apps).
    /// 2. Reject payloads whose `state_root_hash` is not in our GSR history.
    ///    This ensures every transaction is grounded in a state root we have computed ourselves.
    /// 3. Check for duplicate `tx_final` and spent nullifiers before recording the delta.
    ///    Updates are all-or-nothing per payload: either all nullifiers are accepted or none are.
    ///
    /// Note: this method mutates only the provided `WorkingState` plus the provided `SlotDelta`.
    /// Writes happen later through `apply_delta_to_db`, and in-memory state is applied
    /// only after finalize via `apply_delta_to_memory`.
    fn process_blob(
        &self,
        state: &mut WorkingState,
        bytes: &[u8],
        slot: u32,
        block_number: u32,
        delta: &mut SlotDelta,
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

        // A payload is only valid if it references a GSR we have previously computed,
        // and that GSR must be no more than MAX_GSR_AGE_BLOCKS old.
        let Some(&gsr_block) = state.gsr_block_numbers.get(&payload.state_root_hash) else {
            warn!(
                slot,
                block_number,
                "Blob proof state_root_hash not found in known GSR history; rejecting"
            );
            return Ok(());
        };
        let current_block: i64 = block_number.into();
        let age = current_block - gsr_block;
        if age > MAX_GSR_AGE_BLOCKS {
            warn!(
                slot,
                block_number, gsr_block, age, "Blob proof state_root_hash is too old; rejecting"
            );
            return Ok(());
        }

        // All uniqueness checks and DB writes happen under the write lock so that
        // concurrent calls cannot interleave partial state.
        //
        // Strategy: insert tx_final optimistically, then scan all nullifiers.
        // On any collision, roll back the tx_final insertion and bail without touching the DB.
        // Only after all checks pass do we write nullifiers to the DB.
        if !state.transactions.insert(payload.tx_final) {
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
                state.transactions.remove(&payload.tx_final);
                return Ok(());
            }
            if state.nullifiers.contains(nullifier) {
                warn!(slot, block_number, "Duplicate nullifier; rejecting");
                // Roll back the optimistic tx_final insertion.
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
        let mut delta = SlotDelta::default();

        for (blob_index, bytes) in blob_payloads {
            self.process_blob(&mut working, bytes, slot, block_number, &mut delta)
                .with_context(|| {
                    format!(
                        "Failed to process blob at slot {}, blob_index {}",
                        slot, blob_index
                    )
                })?;
        }

        let new_gsr = StateRoot::new(
            block_number as i64,
            &working.transactions,
            &working.nullifiers,
            &working.global_state_roots,
        )
        .hash();
        working.global_state_roots.push(new_gsr);
        delta.gsr_block_numbers.push(block_number);
        delta.gsr_hashes.push(new_gsr);

        info!(
            slot,
            block_number,
            gsr_count = working.global_state_roots.len(),
            "Slot data"
        );

        Ok(delta)
    }

    pub fn apply_delta_to_memory(&self, delta: &SlotDelta) -> Result<()> {
        let mut state = self.write_state()?;
        for tx_hash in &delta.tx_hashes {
            state.transactions.insert(*tx_hash);
        }
        for nullifier in &delta.nullifiers {
            state.nullifiers.insert(*nullifier);
        }
        for gsr in &delta.gsr_hashes {
            state.global_state_roots.push(*gsr);
        }
        for (gsr, block_number) in delta.gsr_hashes.iter().zip(delta.gsr_block_numbers.iter()) {
            state.gsr_block_numbers.insert(*gsr, *block_number as i64);
        }
        Ok(())
    }

    pub fn apply_delta_to_db(&self, delta: &SlotDelta) -> Result<()> {
        self.app_db.apply_delta(
            &delta.tx_hashes,
            &delta.nullifiers,
            &delta.gsr_block_numbers,
            &delta.gsr_hashes,
        )
    }

    pub fn apply_journal(&self, journal: &SlotJournal) -> Result<()> {
        self.app_db.apply_delta(
            &journal.tx_hashes,
            &journal.nullifiers,
            &journal.gsr_block_numbers,
            &journal.gsr_hashes,
        )
    }

    pub fn rollback_journals(&self, journals: &[SlotJournal]) -> Result<()> {
        for journal in journals {
            self.app_db.delete_slot_delta(
                &journal.tx_hashes,
                &journal.nullifiers,
                &journal.gsr_block_numbers,
            )?;
        }
        self.reload_from_db()
    }

    /// Returns `(transactions, nullifiers, global_state_roots)` as owned vecs.
    /// Primarily used in tests; callers that need only one field should add a dedicated accessor.
    #[allow(dead_code)]
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
            "Current state"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_db::AppDb;
    use crate::proof::MockBlobParser;
    use hex::ToHex;
    use pod2::middleware::{hash_values, Value};
    use tempfile::TempDir;
    use txlib::new_obj;

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

    fn seed_gsr0(sm: &StateMachine) -> Hash {
        let d = sm.derive_slot_delta(0, 0, &[]).unwrap();
        sm.apply_delta_to_db(&d).unwrap();
        sm.apply_delta_to_memory(&d).unwrap();
        sm.state_snapshot().unwrap().2[0]
    }

    fn process_and_commit_blob(
        sm: &StateMachine,
        blob: &[u8],
        slot: u32,
        block_number: u32,
    ) -> SlotDelta {
        let d = sm
            .derive_slot_delta(slot, block_number, &[(0, blob.to_vec())])
            .unwrap();
        sm.apply_delta_to_db(&d).unwrap();
        sm.apply_delta_to_memory(&d).unwrap();
        d
    }

    fn advance_and_commit(sm: &StateMachine, slot: u32, block_number: u32) -> SlotDelta {
        let d = sm.derive_slot_delta(slot, block_number, &[]).unwrap();
        sm.apply_delta_to_db(&d).unwrap();
        sm.apply_delta_to_memory(&d).unwrap();
        d
    }

    #[test]
    fn test_happy_path_single_tx() {
        let (sm, _dir) = make_sm();
        let gsr0 = seed_gsr0(&sm);

        let tx_hash = unique_hash(1);
        let nullifier = unique_hash(2);
        process_and_commit_blob(&sm, &mock_txn_bytes(tx_hash, &[nullifier], gsr0), 1, 1);

        let (txns, nullifiers, _) = sm.state_snapshot().unwrap();
        assert!(txns.contains(&tx_hash));
        assert!(nullifiers.contains(&nullifier));
    }

    #[test]
    fn test_sequence_across_blocks() {
        let (sm, _dir) = make_sm();
        let gsr0 = seed_gsr0(&sm);

        let tx1 = unique_hash(1);
        let null1 = unique_hash(2);
        process_and_commit_blob(&sm, &mock_txn_bytes(tx1, &[null1], gsr0), 1, 1);

        let gsr1 = sm.state_snapshot().unwrap().2[1];
        assert_ne!(gsr0, gsr1);

        let tx2 = unique_hash(3);
        let null2 = unique_hash(4);
        process_and_commit_blob(&sm, &mock_txn_bytes(tx2, &[null2], gsr1), 2, 2);

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
        let gsr0 = seed_gsr0(&sm);

        let tx1 = unique_hash(1);
        process_and_commit_blob(&sm, &mock_txn_bytes(tx1, &[], gsr0), 1, 1);

        // tx2 is grounded against gsr0, not the newer gsr1 — still valid
        let tx2 = unique_hash(2);
        process_and_commit_blob(&sm, &mock_txn_bytes(tx2, &[], gsr0), 1, 1);

        let (txns, _, _) = sm.state_snapshot().unwrap();
        assert!(txns.contains(&tx1));
        assert!(txns.contains(&tx2));
    }

    #[test]
    fn test_duplicate_tx_rejected() {
        let (sm, _dir) = make_sm();
        let gsr0 = seed_gsr0(&sm);

        let tx_final = unique_hash(1);
        let bytes = mock_txn_bytes(tx_final, &[], gsr0);

        process_and_commit_blob(&sm, &bytes, 1, 1);
        process_and_commit_blob(&sm, &bytes, 1, 1); // duplicate; silently rejected

        let (txns, _, _) = sm.state_snapshot().unwrap();
        assert_eq!(txns.len(), 1);
    }

    #[test]
    fn test_duplicate_nullifier_rejected() {
        let (sm, _dir) = make_sm();
        let gsr0 = seed_gsr0(&sm);

        let null = unique_hash(10);

        let tx1 = unique_hash(1);
        process_and_commit_blob(&sm, &mock_txn_bytes(tx1, &[null], gsr0), 1, 1);

        let tx2 = unique_hash(2);
        process_and_commit_blob(&sm, &mock_txn_bytes(tx2, &[null], gsr0), 1, 1); // rejected

        let (txns, nullifiers, _) = sm.state_snapshot().unwrap();
        assert!(txns.contains(&tx1));
        assert!(!txns.contains(&tx2));
        assert_eq!(nullifiers.len(), 1);
    }

    #[test]
    fn test_duplicate_nullifier_within_payload_rejected() {
        let (sm, _dir) = make_sm();
        let gsr0 = seed_gsr0(&sm);

        let tx = unique_hash(1);
        let nullifier = unique_hash(10);
        process_and_commit_blob(
            &sm,
            &mock_txn_bytes(tx, &[nullifier, nullifier], gsr0),
            1,
            1,
        );

        let (txns, nullifiers, _) = sm.state_snapshot().unwrap();
        assert!(!txns.contains(&tx));
        assert!(!nullifiers.contains(&nullifier));
    }

    #[test]
    fn test_nullifier_collision_is_atomic() {
        let (sm, _dir) = make_sm();
        let gsr0 = seed_gsr0(&sm);

        let spent = unique_hash(10);
        let fresh_a = unique_hash(11);
        let fresh_b = unique_hash(12);

        let tx1 = unique_hash(1);
        process_and_commit_blob(&sm, &mock_txn_bytes(tx1, &[spent], gsr0), 1, 1);

        // tx2 has [fresh_a, spent, fresh_b] — 'spent' is a duplicate
        let tx2 = unique_hash(2);
        process_and_commit_blob(
            &sm,
            &mock_txn_bytes(tx2, &[fresh_a, spent, fresh_b], gsr0),
            1,
            1,
        );

        let (txns, nullifiers, _) = sm.state_snapshot().unwrap();
        assert!(!txns.contains(&tx2));
        assert!(!nullifiers.contains(&fresh_a));
        assert!(!nullifiers.contains(&fresh_b));
    }

    #[test]
    fn test_unknown_gsr_rejected() {
        let (sm, _dir) = make_sm();
        seed_gsr0(&sm);

        let bogus_gsr = unique_hash(999);
        let tx_final = unique_hash(1);
        process_and_commit_blob(&sm, &mock_txn_bytes(tx_final, &[], bogus_gsr), 1, 1);

        let (txns, _, _) = sm.state_snapshot().unwrap();
        assert!(txns.is_empty());
    }

    #[test]
    fn test_stale_gsr_rejected() {
        let (sm, _dir) = make_sm();
        let gsr0 = seed_gsr0(&sm);

        // Advance 301 more blocks so gsr0 is 301 blocks old when the blob arrives.
        for i in 1..=301 {
            advance_and_commit(&sm, i, i);
        }

        let tx = unique_hash(1);
        process_and_commit_blob(&sm, &mock_txn_bytes(tx, &[], gsr0), 0, 301);

        let (txns, _, _) = sm.state_snapshot().unwrap();
        assert!(txns.is_empty());
    }

    #[test]
    fn test_gsr_at_limit_accepted() {
        let (sm, _dir) = make_sm();
        let gsr0 = seed_gsr0(&sm);

        // Advance 300 more blocks so gsr0 is exactly 300 blocks old — at the limit.
        for i in 1..=300 {
            advance_and_commit(&sm, i, i);
        }

        let tx = unique_hash(1);
        process_and_commit_blob(&sm, &mock_txn_bytes(tx, &[], gsr0), 0, 300);

        let (txns, _, _) = sm.state_snapshot().unwrap();
        assert!(txns.contains(&tx));
    }

    #[test]
    fn test_invalid_blob_skipped() {
        let (sm, _dir) = make_sm();
        seed_gsr0(&sm);

        process_and_commit_blob(&sm, b"not json", 1, 1);

        let (txns, nullifiers, _) = sm.state_snapshot().unwrap();
        assert!(txns.is_empty());
        assert!(nullifiers.is_empty());
    }

    #[test]
    fn test_proof_parse_error_skipped() {
        let (sm, _dir) = make_sm();
        seed_gsr0(&sm);

        // JSON shape matches mock parser, but hash decoding fails, causing parser error.
        process_and_commit_blob(
            &sm,
            br#"{"tx_final":"zz","nullifiers":[],"state_root_hash":"zz"}"#,
            1,
            1,
        );

        let (txns, nullifiers, _) = sm.state_snapshot().unwrap();
        assert!(txns.is_empty());
        assert!(nullifiers.is_empty());
    }

    #[test]
    fn test_rollback_reloads_gsrs_from_retained_slot() {
        let (sm, _dir) = make_sm();
        seed_gsr0(&sm);
        let _g1 = advance_and_commit(&sm, 1, 1);
        let g2 = advance_and_commit(&sm, 2, 2);
        assert_eq!(sm.state_snapshot().unwrap().2.len(), 3);

        let journals = vec![SlotJournal {
            slot: 2,
            tx_hashes: vec![],
            nullifiers: vec![],
            gsr_block_numbers: g2.gsr_block_numbers,
            gsr_hashes: g2.gsr_hashes,
        }];
        sm.rollback_journals(&journals).unwrap();

        let (_, _, gsrs) = sm.state_snapshot().unwrap();
        assert_eq!(gsrs.len(), 2);
    }

    #[test]
    fn test_reorg_rollback_restores_in_memory_sets() {
        let (sm, _dir) = make_sm();
        let gsr0 = seed_gsr0(&sm);

        let tx1 = unique_hash(101);
        let n1 = unique_hash(201);
        process_and_commit_blob(&sm, &mock_txn_bytes(tx1, &[n1], gsr0), 1, 1);
        let gsr1 = sm.state_snapshot().unwrap().2[1];

        let tx2 = unique_hash(102);
        let n2 = unique_hash(202);
        let d2 = process_and_commit_blob(&sm, &mock_txn_bytes(tx2, &[n2], gsr1), 2, 2);
        let g2_gsr_block_numbers = d2.gsr_block_numbers.clone();
        let g2_gsr_hashes = d2.gsr_hashes.clone();

        let journals = vec![SlotJournal {
            slot: 2,
            tx_hashes: d2.tx_hashes,
            nullifiers: d2.nullifiers,
            gsr_block_numbers: g2_gsr_block_numbers,
            gsr_hashes: g2_gsr_hashes,
        }];
        sm.rollback_journals(&journals).unwrap();

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
        use txlib::TxBuilder;

        let params = Params::default();
        let vd_set = &*DEFAULT_VD_SET;
        let shrunk_main_pod_build = ShrunkMainPodSetup::new(&params).build().unwrap();

        let dir = TempDir::new().unwrap();
        let app_db = AppDb::connect(dir.path().to_str().unwrap()).unwrap();
        let sm =
            StateMachine::new(app_db, Arc::new(crate::proof::ProofParser::new().unwrap())).unwrap();

        let gsr0 = seed_gsr0(&sm);

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
        let builder = MultiPodBuilder::new(&params, vd_set);
        let mut ctx = BuildContext {
            builder,
            modules: txlib_modules,
        };

        let obj = new_obj();
        let mut tx_builder = TxBuilder::new(&mut ctx, &[], state_root);
        tx_builder.insert(&mut ctx, obj);
        let (st_finalized, tx) = tx_builder.finalize(&mut ctx);
        ctx.builder.reveal(&st_finalized).unwrap();

        let solution = ctx.builder.solve().unwrap();
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
        process_and_commit_blob(&sm, &payload.to_bytes(), 1, 1);

        let (txns, spent_nullifiers, _) = sm.state_snapshot().unwrap();
        assert!(txns.contains(&tx_final));
        for n in &nullifiers {
            assert!(spent_nullifiers.contains(n));
        }
    }
}
