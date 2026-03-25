use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use anyhow::{Context, Result};
use common::{encode_hash_hex, proof::BlobParser};
use pod2::{
    backends::plonky2::primitives::merkletree::MerkleProof,
    middleware::{containers::Array, containers::Set, Hash, Value},
};
use tracing::{info, warn};

use crate::{
    app_db::AppDb,
    head::{CanonicalHead, CanonicalRoots, HeadMetadata},
};
use txlib::StateRoot;

/// The maximum age of a GSR used as grounding for a transaction.
/// At one block per 12 seconds, this is one hour.
pub const MAX_GSR_AGE_BLOCKS: i64 = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Head transition derived for a single canonical slot.
pub struct SlotDelta {
    /// Head snapshot after processing the slot.
    pub new_head: CanonicalHead,
}

#[derive(Debug, Clone)]
/// Membership proof for a source transaction against the current transactions set root.
pub struct TxMembershipProof {
    /// Source transaction hash the client asked about.
    pub tx_hash: Hash,
    /// Whether the transaction is present in the committed transactions set.
    pub present: bool,
    /// Merkle proof against the current transactions set root.
    pub proof: MerkleProof,
}

#[derive(Debug, Clone)]
/// Proof-bearing result used by txlib to ground action execution.
pub struct GroundingWitnessSnapshot {
    /// Per-source transaction membership proofs under the provided head.
    pub source_tx_proofs: Vec<TxMembershipProof>,
}

#[derive(Debug, Clone)]
/// Membership result anchored to one caller-provided head.
pub struct MembershipSnapshot {
    /// Per-request transaction membership bits under `roots.transactions`.
    pub tx_present: Vec<bool>,
    /// Per-request nullifier membership bits under `roots.nullifiers`.
    pub nullifier_present: Vec<bool>,
}

/// Ephemeral mutable view used while deriving one slot.
///
/// This view opens the persistent POD2 containers used during slot derivation so validation can
/// query and mutate transactions, nullifiers, and GSR history for one slot.
struct WorkingState {
    /// Head snapshot the slot derivation started from.
    head: CanonicalHead,
    /// Persistent transactions set opened from `head.roots.transactions`.
    transactions: Set,
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

    pub fn noop_delta(&self, base_head: CanonicalHead) -> SlotDelta {
        SlotDelta {
            new_head: base_head,
        }
    }

    fn snapshot_working_state(
        &self,
        base_head: CanonicalHead,
        recent_gsrs: HashMap<Hash, i64>,
    ) -> Result<WorkingState> {
        Ok(WorkingState {
            head: base_head,
            transactions: self
                .app_db
                .open_transactions(base_head.roots.transactions)?,
            nullifiers: self.app_db.open_nullifiers(base_head.roots.nullifiers)?,
            gsr_history: self.app_db.open_gsr_history(base_head.roots.gsr_history)?,
            recent_gsrs,
        })
    }

    #[cfg(test)]
    pub fn tx_exists(&self, roots: &CanonicalRoots, tx_hash: &Hash) -> Result<bool> {
        Ok(self
            .membership_snapshot(roots, std::slice::from_ref(tx_hash), &[])?
            .tx_present[0])
    }

    #[cfg(test)]
    pub fn nullifier_exists_batch(
        &self,
        roots: &CanonicalRoots,
        nullifiers: &[Hash],
    ) -> Result<Vec<bool>> {
        Ok(self
            .membership_snapshot(roots, &[], nullifiers)?
            .nullifier_present)
    }

    pub fn membership_snapshot(
        &self,
        roots: &CanonicalRoots,
        tx_hashes: &[Hash],
        nullifiers: &[Hash],
    ) -> Result<MembershipSnapshot> {
        Ok(MembershipSnapshot {
            tx_present: self.app_db.tx_exists_batch(roots, tx_hashes)?,
            nullifier_present: self.app_db.nullifier_exists_batch(roots, nullifiers)?,
        })
    }

    pub fn grounding_witness(
        &self,
        roots: &CanonicalRoots,
        source_tx_hashes: &[Hash],
    ) -> Result<GroundingWitnessSnapshot> {
        let source_tx_proofs = source_tx_hashes
            .iter()
            .map(|tx_hash| {
                let (present, proof) = self.app_db.prove_tx(roots, *tx_hash)?;
                Ok(TxMembershipProof {
                    tx_hash: *tx_hash,
                    present,
                    proof,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(GroundingWitnessSnapshot { source_tx_proofs })
    }

    /// Parse and validate one blob payload against the in-progress slot state.
    ///
    /// This is the core per-blob derivation step used by `derive_slot_delta`.
    /// The method is intentionally fail-soft for invalid blob contents: malformed payloads,
    /// proofs grounded in unknown/too-old GSRs, duplicate transactions, and duplicate
    /// nullifiers are logged and skipped rather than aborting the whole slot.
    ///
    /// Validation order:
    /// 1. Parse and verify the blob as a `TxFinalized` payload via `proof_parser`
    /// 2. Check that `payload.state_root_hash` exists in the recent canonical GSR cache
    /// 3. Enforce the maximum grounding age window (`MAX_GSR_AGE_BLOCKS`)
    /// 4. Reject duplicate `tx_final` values already present in the transactions set
    /// 5. Reject duplicate nullifiers both within the payload and against the canonical
    ///    nullifiers set
    ///
    /// If all checks pass, the method mutates the slot-local `WorkingState` by:
    /// - inserting `tx_final` into the persistent transactions set handle
    /// - inserting each nullifier into the persistent nullifiers set handle
    /// - incrementing the corresponding counts in `state.head`
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
        state.head.metadata.tx_count += 1;
        for nullifier in &payload.nullifiers {
            state.nullifiers.insert(&Value::from(*nullifier))?;
            state.head.metadata.nullifier_count += 1;
        }

        info!(
            slot,
            block_number,
            transaction_count = state.head.metadata.tx_count,
            nullifier_count = state.head.metadata.nullifier_count,
            "Validated blob state update in slot derivation"
        );
        Ok(())
    }

    pub fn derive_slot_delta(
        &self,
        base_head: CanonicalHead,
        recent_gsrs: impl IntoIterator<Item = (Hash, i64)>,
        slot: u32,
        block_number: u32,
        blob_payloads: &[(u32, Vec<u8>)],
    ) -> Result<SlotDelta> {
        let mut working =
            self.snapshot_working_state(base_head, recent_gsrs.into_iter().collect())?;

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
            working.transactions.commitment(),
            working.nullifiers.commitment(),
            prior_gsrs_root,
        )
        .hash();

        working
            .gsr_history
            .insert(base_head.metadata.gsr_count as usize, Value::from(new_gsr))?;
        working.recent_gsrs.insert(new_gsr, i64::from(block_number));

        let min_block = i64::from(block_number) - MAX_GSR_AGE_BLOCKS;
        working
            .recent_gsrs
            .retain(|_, seen_block| *seen_block >= min_block);

        let new_head = CanonicalHead {
            roots: CanonicalRoots {
                transactions: working.transactions.commitment(),
                nullifiers: working.nullifiers.commitment(),
                state_root_gsrs: prior_gsrs_root,
                gsr_history: working.gsr_history.commitment(),
            },
            metadata: HeadMetadata {
                current_gsr: Some(new_gsr),
                current_block_number: Some(block_number),
                tx_count: working.head.metadata.tx_count,
                nullifier_count: working.head.metadata.nullifier_count,
                gsr_count: base_head.metadata.gsr_count + 1,
            },
        };

        info!(
            slot,
            block_number,
            gsr_count = new_head.metadata.gsr_count,
            "Slot data"
        );

        Ok(SlotDelta { new_head })
    }

    pub fn log_current_state(&self, head: CanonicalHead) {
        let current_gsr = head
            .metadata
            .current_gsr
            .as_ref()
            .map(encode_hash_hex)
            .unwrap_or_else(|| "none".to_string());
        info!(
            transaction_count = head.metadata.tx_count,
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

    fn make_sm() -> (StateMachine, TempDir) {
        let dir = TempDir::new().unwrap();
        let app_db = AppDb::connect(dir.path().to_str().unwrap()).unwrap();
        let sm = StateMachine::new(app_db, Arc::new(MockBlobParser));
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

    fn seed_gsr0(sm: &StateMachine) -> CanonicalHead {
        sm.derive_slot_delta(CanonicalHead::empty(), [], 0, 0, &[])
            .unwrap()
            .new_head
    }

    #[test]
    fn test_empty_slot_produces_new_head() {
        let (sm, _dir) = make_sm();
        let head = sm
            .derive_slot_delta(CanonicalHead::empty(), [], 1, 7, &[])
            .unwrap()
            .new_head;
        assert_eq!(head.metadata.current_block_number, Some(7));
        assert_eq!(head.metadata.gsr_count, 1);
    }

    #[test]
    fn test_accepts_valid_blob_and_updates_counts() {
        let (sm, _dir) = make_sm();
        let head0 = seed_gsr0(&sm);
        let gsr0 = head0.metadata.current_gsr.unwrap();

        let tx_final = unique_hash(10);
        let nullifier = unique_hash(11);
        let blob = mock_txn_bytes(tx_final, &[nullifier], gsr0);
        let head1 = sm
            .derive_slot_delta(head0, [(gsr0, 0)], 1, 1, &[(0, blob)])
            .unwrap()
            .new_head;

        assert_eq!(head1.metadata.tx_count, 1);
        assert_eq!(head1.metadata.nullifier_count, 1);
        assert_eq!(head1.metadata.gsr_count, 2);
        assert!(sm.tx_exists(&head1.roots, &tx_final).unwrap());
        assert_eq!(
            sm.nullifier_exists_batch(&head1.roots, &[nullifier])
                .unwrap(),
            vec![true]
        );
    }

    #[test]
    fn test_rejects_unknown_grounding_gsr() {
        let (sm, _dir) = make_sm();
        let head0 = seed_gsr0(&sm);

        let tx_final = unique_hash(21);
        let blob = mock_txn_bytes(tx_final, &[], unique_hash(99));
        let head1 = sm
            .derive_slot_delta(head0, [], 2, 2, &[(0, blob)])
            .unwrap()
            .new_head;
        assert_eq!(head1.metadata.tx_count, head0.metadata.tx_count);
    }

    #[test]
    fn test_uncommitted_derived_nodes_do_not_affect_old_head() {
        let (sm, _dir) = make_sm();
        let head0 = seed_gsr0(&sm);
        let gsr0 = head0.metadata.current_gsr.unwrap();

        let tx_final = unique_hash(31);
        let blob = mock_txn_bytes(tx_final, &[], gsr0);
        let head1 = sm
            .derive_slot_delta(head0, [(gsr0, 0)], 1, 1, &[(0, blob)])
            .unwrap()
            .new_head;

        assert_eq!(sm.tx_exists(&head0.roots, &tx_final).unwrap(), false);
        assert_eq!(sm.tx_exists(&head1.roots, &tx_final).unwrap(), true);
    }
}
