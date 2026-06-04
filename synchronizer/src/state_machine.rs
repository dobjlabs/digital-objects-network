use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use anyhow::{Context, Result};
use common::{encode_hash_hex, proof::BlobParser};
use pod2::middleware::{containers::Array, containers::Set, Hash, Value};
use tracing::{info, warn};

use crate::{
    app_db::AppDb,
    head::{CanonicalHead, CanonicalRoots, HeadMetadata},
};
use txlib::StateRoot;

/// The maximum age of a GSR used as grounding for a transaction.
/// At one block per 12 seconds, this is one hour.
pub const MAX_GSR_AGE_BLOCKS: i64 = 300;

/// Ephemeral mutable view used while deriving one slot.
///
/// This view opens the persistent POD2 containers used during slot derivation so validation can
/// query and mutate the created-object set, nullifiers, and GSR history for one slot.
struct WorkingState {
    /// Mutable non-root metadata accumulated while deriving the slot.
    metadata: HeadMetadata,
    /// Persistent global created-object set (a pod2 `Array`) opened from
    /// `head.roots.created`.
    created: Array,
    /// Persistent nullifiers set opened from `head.roots.nullifiers`.
    nullifiers: Set,
    /// Persistent full GSR history array opened from `head.roots.gsr_history`.
    gsr_history: Array,
    /// Recent canonical GSRs keyed by hash for grounding validation.
    recent_gsrs: HashMap<Hash, i64>,
}

/// Domain logic for the synchronizer: proof verification, state validation, and Merkle storage.
///
/// `StateMachine` is intentionally decoupled from networking and canonical-head ownership.
/// Callers supply the `CanonicalHead` they want to operate against, and Postgres remains the sole
/// source of truth for which head is canonical.
pub struct StateMachine {
    /// RocksDB-backed app-state store used to open persistent containers and prove membership.
    app_db: AppDb,
    /// Blob parser/verifier used to decode TxFinalized payloads from blob bytes.
    proof_parser: Arc<dyn BlobParser>,
}

impl StateMachine {
    pub fn new(app_db: AppDb, proof_parser: Arc<dyn BlobParser>) -> Self {
        Self {
            app_db,
            proof_parser,
        }
    }

    /// Parse and validate one blob payload against the in-progress slot state.
    ///
    /// This is the core per-blob derivation step used by `derive_slot_head`.
    /// The method is intentionally fail-soft for invalid blob contents: malformed payloads,
    /// proofs grounded in unknown/too-old GSRs, duplicate transactions, and duplicate
    /// nullifiers are logged and skipped rather than aborting the whole slot.
    ///
    /// Validation order:
    /// 1. Parse and verify the blob as a `TxFinalized` payload via `proof_parser`
    /// 2. Check that `payload.state_root_hash` exists in the recent canonical GSR cache
    /// 3. Enforce the maximum grounding age window (`MAX_GSR_AGE_BLOCKS`)
    /// 4. Reject duplicate created-object commitments both within the payload and against the
    ///    in-progress created set: a tx that creates an object that already exists fails the same
    ///    way a nullifier double-spend does. This is also what gives no-input (mining) txs their
    ///    replay protection.
    /// 5. Reject duplicate nullifiers both within the payload and against the in-progress
    ///    nullifiers set, which starts from canonical state and includes earlier accepted blobs
    ///
    /// If all checks pass, the method mutates the slot-local `WorkingState` by:
    /// - appending each created-object commitment to the persistent in-progress created array
    ///   and recording its index in the reverse-index cache
    /// - inserting each nullifier into the persistent in-progress nullifiers set handle
    /// - incrementing the corresponding counts in `state.metadata`
    ///
    /// No new canonical head is committed here. Mutating the persistent container handles may
    /// materialize Merkle nodes/values in RocksDB, but the canonical state changes only if the
    /// caller later publishes the resulting `CanonicalHead` in Postgres.
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

        // Pre-check created objects and nullifiers before mutating anything, so a
        // single collision rejects the whole blob with no partial application.
        let mut payload_created = HashSet::with_capacity(payload.live.len());
        for obj in &payload.live {
            if !payload_created.insert(*obj) {
                warn!(
                    slot,
                    block_number, "Duplicate created object within payload; rejecting"
                );
                return Ok(());
            }
            if self.is_created(state, *obj)? {
                warn!(
                    slot,
                    block_number, "Created object already exists (creation collision); rejecting"
                );
                return Ok(());
            }
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

        for obj in &payload.live {
            // The created array is 0-indexed: the next object lands at slot
            // `created_count`, which doubles as the true object count.
            let index = state.metadata.created_count as usize;
            state.created.insert(index, Value::from(*obj))?;
            self.app_db.created_index_put(*obj, index as i64)?;
            state.metadata.created_count += 1;
        }
        for nullifier in &payload.nullifiers {
            state.nullifiers.insert(&Value::from(*nullifier))?;
            state.metadata.nullifier_count += 1;
        }

        info!(
            slot,
            block_number,
            created_count = state.metadata.created_count,
            nullifier_count = state.metadata.nullifier_count,
            "Validated blob state update in slot derivation"
        );
        Ok(())
    }

    /// Whether `commitment` is already in the in-progress created set. The cache
    /// gives a candidate index; the in-progress array is the authority, so a
    /// stale cache hit (pointing at an index this array does not hold, or holds
    /// a different commitment at) correctly reads as "not created".
    fn is_created(&self, state: &WorkingState, commitment: Hash) -> Result<bool> {
        match self.app_db.created_index_get(commitment)? {
            None => Ok(false),
            Some(index) => Ok(state.created.get(index as usize)? == Some(Value::from(commitment))),
        }
    }

    /// Derive the next canonical head for one execution slot from a caller-provided base head.
    ///
    /// This method is the slot-level orchestration layer around `process_blob`.
    /// It:
    /// - reopens the persistent created-object, nullifiers, and GSR-history containers from
    ///   `base_head.roots`
    /// - seeds the per-slot `WorkingState` with the caller-provided recent-GSR window
    /// - feeds every decoded blob payload through `process_blob`, accumulating accepted updates
    ///   in the in-progress container handles
    /// - computes the next GSR from the updated created/nullifiers roots and the prior
    ///   GSR-history root committed into the resulting `StateRoot`
    /// - appends that new GSR to the full history array and returns the resulting `CanonicalHead`
    ///
    /// The returned head is only a candidate next canonical state. By the time this method
    /// returns, RocksDB may already contain Merkle nodes for the derived containers, but the
    /// head does not become canonical until the caller persists it in Postgres.
    ///
    /// Empty or fully rejected slots still produce a new head with the same created and
    /// nullifiers roots as `base_head`, but with a newly derived `current_gsr` and an appended
    /// GSR-history entry for the slot's execution block.
    pub fn derive_slot_head(
        &self,
        base_head: CanonicalHead,
        recent_gsrs: impl IntoIterator<Item = (Hash, i64)>,
        slot: u32,
        block_number: u32,
        blob_payloads: &[(u32, Vec<u8>)],
    ) -> Result<CanonicalHead> {
        let mut working = WorkingState {
            metadata: base_head.metadata,
            created: self.app_db.open_created(base_head.roots.created)?,
            nullifiers: self.app_db.open_nullifiers(base_head.roots.nullifiers)?,
            gsr_history: self.app_db.open_gsr_history(base_head.roots.gsr_history)?,
            recent_gsrs: recent_gsrs.into_iter().collect(),
        };

        for (blob_index, bytes) in blob_payloads {
            self.process_blob(&mut working, bytes, slot, block_number)
                .with_context(|| {
                    format!(
                        "Failed to process blob at slot {}, blob_index {}",
                        slot, blob_index
                    )
                })?;
        }

        let prior_gsrs_root = base_head.roots.gsr_history;
        let new_gsr = StateRoot::new(
            i64::from(block_number),
            working.created.commitment(),
            working.nullifiers.commitment(),
            prior_gsrs_root,
        )
        .hash();

        working
            .gsr_history
            .insert(base_head.metadata.gsr_count as usize, Value::from(new_gsr))?;

        let new_head = CanonicalHead {
            roots: CanonicalRoots {
                created: working.created.commitment(),
                nullifiers: working.nullifiers.commitment(),
                state_root_gsrs: prior_gsrs_root,
                gsr_history: working.gsr_history.commitment(),
            },
            metadata: HeadMetadata {
                current_gsr: Some(new_gsr),
                current_block_number: Some(block_number),
                created_count: working.metadata.created_count,
                nullifier_count: working.metadata.nullifier_count,
                gsr_count: base_head.metadata.gsr_count + 1,
            },
        };

        info!(
            slot,
            block_number,
            gsr_count = new_head.metadata.gsr_count,
            "Slot data"
        );

        Ok(new_head)
    }

    pub fn log_current_state(&self, head: CanonicalHead) {
        let current_gsr = head
            .metadata
            .current_gsr
            .as_ref()
            .map(encode_hash_hex)
            .unwrap_or_else(|| "none".to_string());
        info!(
            created_count = head.metadata.created_count,
            nullifier_count = head.metadata.nullifier_count,
            gsr_count = head.metadata.gsr_count,
            current_gsr = %current_gsr,
            "Current state"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_db::AppDb;
    use common::proof::MockBlobParser;
    use pod2::middleware::hash_values;
    use tempfile::TempDir;

    fn make_sm() -> (StateMachine, AppDb, TempDir) {
        let dir = TempDir::new().unwrap();
        let app_db = AppDb::connect(dir.path().to_str().unwrap()).unwrap();
        let sm = StateMachine::new(app_db.clone(), Arc::new(MockBlobParser));
        (sm, app_db, dir)
    }

    fn unique_hash(n: i64) -> Hash {
        hash_values(&[Value::from(n)])
    }

    fn mock_txn_bytes(
        tx_final: Hash,
        nullifiers: &[Hash],
        live: &[Hash],
        state_root: Hash,
    ) -> Vec<u8> {
        let hashes_json = |hashes: &[Hash]| {
            hashes
                .iter()
                .map(|h| format!("\"{:#}\"", h))
                .collect::<Vec<_>>()
                .join(",")
        };
        format!(
            r#"{{"tx_final":"{:#}","nullifiers":[{}],"live":[{}],"state_root_hash":"{:#}"}}"#,
            tx_final,
            hashes_json(nullifiers),
            hashes_json(live),
            state_root
        )
        .into_bytes()
    }

    fn seed_gsr0(sm: &StateMachine) -> CanonicalHead {
        sm.derive_slot_head(CanonicalHead::empty(), [], 0, 0, &[])
            .unwrap()
    }

    #[test]
    fn test_empty_slot_produces_new_head() {
        let (sm, _app_db, _dir) = make_sm();
        let head = sm
            .derive_slot_head(CanonicalHead::empty(), [], 1, 7, &[])
            .unwrap();
        assert_eq!(head.metadata.current_block_number, Some(7));
        assert_eq!(head.metadata.gsr_count, 1);
    }

    #[test]
    fn test_accepts_valid_blob_and_updates_counts() {
        let (sm, app_db, _dir) = make_sm();
        let head0 = seed_gsr0(&sm);
        let gsr0 = head0.metadata.current_gsr.unwrap();

        let tx_final = unique_hash(10);
        let nullifier = unique_hash(11);
        let live_obj = unique_hash(12);
        let blob = mock_txn_bytes(tx_final, &[nullifier], &[live_obj], gsr0);
        let head1 = sm
            .derive_slot_head(head0, [(gsr0, 0)], 1, 1, &[(0, blob)])
            .unwrap();
        let created_present = app_db
            .created_exists_batch(&head1.roots, &[live_obj])
            .unwrap();
        let nullifier_present = app_db
            .nullifier_exists_batch(&head1.roots, &[nullifier])
            .unwrap();

        assert_eq!(head1.metadata.created_count, 1);
        assert_eq!(head1.metadata.nullifier_count, 1);
        assert_eq!(head1.metadata.gsr_count, 2);
        assert_eq!(created_present, vec![true]);
        assert_eq!(nullifier_present, vec![true]);
    }

    #[test]
    fn test_rejects_unknown_grounding_gsr() {
        let (sm, _app_db, _dir) = make_sm();
        let head0 = seed_gsr0(&sm);

        let tx_final = unique_hash(21);
        let blob = mock_txn_bytes(tx_final, &[], &[unique_hash(22)], unique_hash(99));
        let head1 = sm.derive_slot_head(head0, [], 2, 2, &[(0, blob)]).unwrap();
        assert_eq!(head1.metadata.created_count, head0.metadata.created_count);
    }

    #[test]
    fn test_uncommitted_derived_nodes_do_not_affect_old_head() {
        let (sm, app_db, _dir) = make_sm();
        let head0 = seed_gsr0(&sm);
        let gsr0 = head0.metadata.current_gsr.unwrap();

        let tx_final = unique_hash(31);
        let live_obj = unique_hash(32);
        let blob = mock_txn_bytes(tx_final, &[], &[live_obj], gsr0);
        let head1 = sm
            .derive_slot_head(head0, [(gsr0, 0)], 1, 1, &[(0, blob)])
            .unwrap();
        let old_membership = app_db
            .created_exists_batch(&head0.roots, &[live_obj])
            .unwrap();
        let new_membership = app_db
            .created_exists_batch(&head1.roots, &[live_obj])
            .unwrap();

        assert_eq!(old_membership, vec![false]);
        assert_eq!(new_membership, vec![true]);
    }

    #[test]
    fn test_rejects_duplicate_created_object() {
        let (sm, _app_db, _dir) = make_sm();
        let head0 = seed_gsr0(&sm);
        let gsr0 = head0.metadata.current_gsr.unwrap();

        let live_obj = unique_hash(40);
        let blob1 = mock_txn_bytes(unique_hash(41), &[], &[live_obj], gsr0);
        let head1 = sm
            .derive_slot_head(head0, [(gsr0, 0)], 1, 1, &[(0, blob1)])
            .unwrap();
        assert_eq!(head1.metadata.created_count, 1);

        // A second tx that re-creates the same object is rejected, exactly
        // like a nullifier double-spend.
        let gsr1 = head1.metadata.current_gsr.unwrap();
        let blob2 = mock_txn_bytes(unique_hash(42), &[], &[live_obj], gsr1);
        let head2 = sm
            .derive_slot_head(head1, [(gsr1, 1)], 2, 2, &[(0, blob2)])
            .unwrap();
        assert_eq!(head2.metadata.created_count, head1.metadata.created_count);
    }
}
