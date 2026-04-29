//! Per-slot state derivation: parse blobs, advance the canonical SMTs, and
//! compute the next [`StateRoot`].
//!
//! ## GSR-history shape
//!
//! Unlike the pod2-era synchronizer, the GSR history is a chain hash, not a
//! Merkle array. There's no use case for proving "GSR at index N is X" inside
//! a risc0 receipt — the only thing the system uses GSR history for is the
//! binding `gsrs_root` field inside each new [`StateRoot`]. A chain hash gives
//! us:
//! - O(1) update per slot (`new = SHA256(DOMAIN || prev || gsr)`),
//! - a stable commitment to the entire prior history, and
//! - one less Merkle structure to maintain in RocksDB.
//!
//! `state_root_gsrs` is the chain hash *before* the current slot's GSR is
//! appended (that's what the receipt's grounding proof commits against).
//! `gsr_history` is the chain hash *after* the append (the next slot's
//! `state_root_gsrs`).

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use anyhow::{Context, Result};
use common::{encode_hash_hex, proof::BlobParser};
use tracing::{info, warn};
use txlib_core::Hash;
use txlib_core::hash::sha256_concat;
use txlib_core::merkle_store::PersistentSmt;
use txlib_core::tx::StateRoot;

use crate::{
    app_db::AppDb,
    head::{CanonicalHead, CanonicalRoots, HeadMetadata},
};

/// The maximum age of a GSR used as grounding for a transaction.
/// At one block per 12 seconds, this is one hour.
pub const MAX_GSR_AGE_BLOCKS: i64 = 300;

/// Domain for the GSR-history chain hash.
const GSR_CHAIN_DOMAIN: &[u8; 8] = b"DOBJ-GSC";

/// `SHA256(DOBJ-GSC || prev_chain || new_gsr)`.
pub fn extend_gsr_chain(prev_chain: Hash, new_gsr: Hash) -> Hash {
    sha256_concat(&[GSR_CHAIN_DOMAIN, prev_chain.as_bytes(), new_gsr.as_bytes()])
}

/// Slot-local mutable state. Owns SMT views over the in-progress
/// transactions + nullifiers, plus the count metadata.
struct WorkingState<'a> {
    metadata: HeadMetadata,
    transactions: PersistentSmt<'a, AppDb>,
    nullifiers: PersistentSmt<'a, AppDb>,
    /// `state_root_hash → grounding_block_number` — caller-provided window
    /// of recent canonical GSRs that incoming proofs are allowed to ground
    /// against.
    recent_gsrs: HashMap<Hash, i64>,
}

pub struct StateMachine {
    app_db: AppDb,
    proof_parser: Arc<dyn BlobParser>,
}

impl StateMachine {
    pub fn new(app_db: AppDb, proof_parser: Arc<dyn BlobParser>) -> Self {
        Self {
            app_db,
            proof_parser,
        }
    }

    /// Validate one blob against the in-progress slot state.
    ///
    /// Fail-soft for malformed input: invalid receipts, stale grounding, and
    /// duplicate tx_finals / nullifiers are logged and skipped rather than
    /// aborting the whole slot.
    fn process_blob(
        &self,
        state: &mut WorkingState<'_>,
        bytes: &[u8],
        slot: u32,
        block_number: u32,
    ) -> Result<()> {
        let payload = match self.proof_parser.parse_blob(bytes) {
            Ok(Some(p)) => p,
            Ok(None) => {
                info!(
                    slot,
                    block_number, "Blob did not carry our magic envelope; skipping"
                );
                return Ok(());
            }
            Err(err) => {
                warn!(
                    slot,
                    block_number,
                    ?err,
                    "Failed to verify receipt; skipping blob"
                );
                return Ok(());
            }
        };

        // 1. State root must be in our recent canonical window.
        let Some(&gsr_block) = state.recent_gsrs.get(&payload.state_root_hash) else {
            warn!(
                slot,
                block_number,
                "Blob proof state_root_hash not in recent GSR window; rejecting"
            );
            return Ok(());
        };
        let age = i64::from(block_number) - gsr_block;
        if age > MAX_GSR_AGE_BLOCKS {
            warn!(
                slot,
                block_number, gsr_block, age, "Grounding GSR is too old; rejecting"
            );
            return Ok(());
        }

        // 2. tx_final must not already be present.
        if state
            .transactions
            .contains_set_member(payload.tx_final)
            .map_err(|e| anyhow::anyhow!("{e}"))?
        {
            warn!(slot, block_number, "Duplicate tx_final; rejecting");
            return Ok(());
        }

        // 3. Nullifiers: deduped within the payload, none already present.
        let mut payload_nullifiers = HashSet::with_capacity(payload.nullifiers.len());
        for nullifier in &payload.nullifiers {
            if !payload_nullifiers.insert(*nullifier) {
                warn!(
                    slot,
                    block_number, "Duplicate nullifier within payload; rejecting"
                );
                return Ok(());
            }
            if state
                .nullifiers
                .contains_set_member(*nullifier)
                .map_err(|e| anyhow::anyhow!("{e}"))?
            {
                warn!(slot, block_number, "Duplicate nullifier; rejecting");
                return Ok(());
            }
        }

        // 4. All checks passed — commit the mutations.
        state
            .transactions
            .insert(payload.tx_final, payload.tx_final)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        state.metadata.tx_count += 1;
        for nullifier in &payload.nullifiers {
            state
                .nullifiers
                .insert(*nullifier, *nullifier)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            state.metadata.nullifier_count += 1;
        }

        info!(
            slot,
            block_number,
            tx_count = state.metadata.tx_count,
            nullifier_count = state.metadata.nullifier_count,
            "Validated blob state update"
        );
        Ok(())
    }

    /// Derive the next canonical head from `base_head` after processing the
    /// slot's blob payloads.
    ///
    /// Empty / fully-rejected slots still produce a new head with the same
    /// transactions + nullifiers roots, but with a freshly derived
    /// `current_gsr` and an extended GSR-history chain.
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
            transactions: self.app_db.open_transactions(base_head.roots.transactions),
            nullifiers: self.app_db.open_nullifiers(base_head.roots.nullifiers),
            recent_gsrs: recent_gsrs.into_iter().collect(),
        };

        for (blob_index, bytes) in blob_payloads {
            self.process_blob(&mut working, bytes, slot, block_number)
                .with_context(|| {
                    format!("Failed to process blob at slot {slot}, blob_index {blob_index}")
                })?;
        }

        let prior_gsrs_chain = base_head.roots.gsr_history;
        let new_gsr = StateRoot::new(
            i64::from(block_number),
            working.transactions.root,
            working.nullifiers.root,
            prior_gsrs_chain,
        )
        .hash();
        let next_gsr_chain = extend_gsr_chain(prior_gsrs_chain, new_gsr);

        let new_head = CanonicalHead {
            roots: CanonicalRoots {
                transactions: working.transactions.root,
                nullifiers: working.nullifiers.root,
                state_root_gsrs: prior_gsrs_chain,
                gsr_history: next_gsr_chain,
            },
            metadata: HeadMetadata {
                current_gsr: Some(new_gsr),
                current_block_number: Some(block_number),
                tx_count: working.metadata.tx_count,
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
            tx_count = head.metadata.tx_count,
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
    use tempfile::TempDir;
    use txlib_core::hash::sha256;

    fn make_sm() -> (StateMachine, AppDb, TempDir) {
        let dir = TempDir::new().unwrap();
        let app_db = AppDb::connect(dir.path().to_str().unwrap()).unwrap();
        let sm = StateMachine::new(app_db.clone(), Arc::new(MockBlobParser));
        (sm, app_db, dir)
    }

    fn hex_lower(b: &[u8]) -> String {
        let mut s = String::with_capacity(b.len() * 2);
        for byte in b {
            s.push_str(&format!("{byte:02x}"));
        }
        s
    }

    fn mock_payload_blob(tx_final: Hash, nullifiers: &[Hash], state_root: Hash) -> Vec<u8> {
        let null_strs: Vec<String> = nullifiers
            .iter()
            .map(|h| format!("\"0x{}\"", hex_lower(h.as_bytes())))
            .collect();
        let json = format!(
            r#"{{"tx_final":"0x{}","nullifiers":[{}],"state_root_hash":"0x{}"}}"#,
            hex_lower(tx_final.as_bytes()),
            null_strs.join(","),
            hex_lower(state_root.as_bytes())
        );
        common::payload::encode_blob_payload(json.as_bytes())
    }

    fn seed_gsr0(sm: &StateMachine) -> CanonicalHead {
        sm.derive_slot_head(CanonicalHead::empty(), [], 0, 0, &[])
            .unwrap()
    }

    #[test]
    fn empty_slot_produces_new_head() {
        let (sm, _app_db, _dir) = make_sm();
        let head = sm
            .derive_slot_head(CanonicalHead::empty(), [], 1, 7, &[])
            .unwrap();
        assert_eq!(head.metadata.current_block_number, Some(7));
        assert_eq!(head.metadata.gsr_count, 1);
    }

    #[test]
    fn accepts_valid_blob_and_updates_counts() {
        let (sm, app_db, _dir) = make_sm();
        let head0 = seed_gsr0(&sm);
        let gsr0 = head0.metadata.current_gsr.unwrap();

        let tx_final = sha256(b"tx10");
        let nullifier = sha256(b"n11");
        let blob = mock_payload_blob(tx_final, &[nullifier], gsr0);
        let head1 = sm
            .derive_slot_head(head0, [(gsr0, 0)], 1, 1, &[(0, blob)])
            .unwrap();

        assert_eq!(head1.metadata.tx_count, 1);
        assert_eq!(head1.metadata.nullifier_count, 1);
        assert_eq!(head1.metadata.gsr_count, 2);
        assert_eq!(
            app_db.tx_exists_batch(&head1.roots, &[tx_final]).unwrap(),
            vec![true]
        );
        assert_eq!(
            app_db
                .nullifier_exists_batch(&head1.roots, &[nullifier])
                .unwrap(),
            vec![true]
        );
    }

    #[test]
    fn rejects_unknown_grounding_gsr() {
        let (sm, _app_db, _dir) = make_sm();
        let head0 = seed_gsr0(&sm);

        let blob = mock_payload_blob(sha256(b"tx21"), &[], sha256(b"unknown gsr"));
        let head1 = sm.derive_slot_head(head0, [], 2, 2, &[(0, blob)]).unwrap();
        assert_eq!(head1.metadata.tx_count, head0.metadata.tx_count);
    }

    #[test]
    fn old_head_unaffected_by_new_inserts() {
        let (sm, app_db, _dir) = make_sm();
        let head0 = seed_gsr0(&sm);
        let gsr0 = head0.metadata.current_gsr.unwrap();

        let tx_final = sha256(b"tx31");
        let blob = mock_payload_blob(tx_final, &[], gsr0);
        let head1 = sm
            .derive_slot_head(head0, [(gsr0, 0)], 1, 1, &[(0, blob)])
            .unwrap();

        assert_eq!(
            app_db.tx_exists_batch(&head0.roots, &[tx_final]).unwrap(),
            vec![false]
        );
        assert_eq!(
            app_db.tx_exists_batch(&head1.roots, &[tx_final]).unwrap(),
            vec![true]
        );
    }

    #[test]
    fn rejects_duplicate_tx_final_in_same_slot() {
        let (sm, _app_db, _dir) = make_sm();
        let head0 = seed_gsr0(&sm);
        let gsr0 = head0.metadata.current_gsr.unwrap();

        let tx = sha256(b"dup");
        let b1 = mock_payload_blob(tx, &[sha256(b"n1")], gsr0);
        let b2 = mock_payload_blob(tx, &[sha256(b"n2")], gsr0);
        let head = sm
            .derive_slot_head(head0, [(gsr0, 0)], 1, 1, &[(0, b1), (1, b2)])
            .unwrap();
        assert_eq!(head.metadata.tx_count, 1);
        // Only the first blob's nullifier was accepted.
        assert_eq!(head.metadata.nullifier_count, 1);
    }

    #[test]
    fn gsr_chain_advances_each_slot() {
        let (sm, _app_db, _dir) = make_sm();
        let h0 = sm
            .derive_slot_head(CanonicalHead::empty(), [], 0, 0, &[])
            .unwrap();
        let h1 = sm.derive_slot_head(h0, [], 1, 1, &[]).unwrap();
        let h2 = sm.derive_slot_head(h1, [], 2, 2, &[]).unwrap();
        assert_ne!(h0.roots.gsr_history, h1.roots.gsr_history);
        assert_ne!(h1.roots.gsr_history, h2.roots.gsr_history);
        // state_root_gsrs of slot N+1 == gsr_history of slot N.
        assert_eq!(h1.roots.state_root_gsrs, h0.roots.gsr_history);
        assert_eq!(h2.roots.state_root_gsrs, h1.roots.gsr_history);
    }
}
