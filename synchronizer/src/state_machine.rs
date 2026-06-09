use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use anyhow::{Context, Result};
use common::{payload::Payload, proof::BlobParser};
use pod2::middleware::{containers::Array, containers::Set, Hash, Value};
use tracing::{info, warn};

use crate::{
    app_db::{created_array_holds, AppDb},
    head::{StateHead, StateMetadata, StateRoots},
};
use txlib::StateHeader;

/// The maximum age of a state root used as grounding for a transaction.
/// At one block per 12 seconds, this is one hour.
pub const MAX_STATE_ROOT_AGE_BLOCKS: i64 = 300;

/// Ephemeral mutable view used while deriving one slot.
///
/// This view opens the persistent POD2 containers used during slot derivation so validation can
/// query and mutate the created-object set, nullifiers, and state root history for one slot.
struct WorkingState {
    /// Mutable non-root metadata accumulated while deriving the slot.
    metadata: StateMetadata,
    /// Persistent global created-object set (a pod2 `Array`) opened from
    /// `head.roots.created`.
    created: Array,
    /// Persistent nullifiers set opened from `head.roots.nullifiers`.
    nullifiers: Set,
    /// Persistent full state root history array opened from `head.roots.next_state_history`.
    next_state_history: Array,
    /// Recent state roots keyed by hash for grounding validation.
    recent_state_roots: HashMap<Hash, i64>,
    /// Commitments this slot appended to `created`, keyed to their array
    /// indices. Doubles as the within-slot duplicate check: the persistent
    /// created index reflects only committed slots, so it cannot see objects
    /// added earlier in this same slot. The caller persists these entries to
    /// the created index in the slot's commit transaction.
    created_added: HashMap<Hash, i64>,
}

/// One slot's derivation result: the candidate next state head plus the
/// object commitments (with array indices) it appended to `created`. The caller
/// persists the additions to the created index when committing the slot.
pub struct DerivedSlot {
    pub head: StateHead,
    pub created_added: HashMap<Hash, i64>,
}

/// Domain logic for the synchronizer: proof verification, state validation, and Merkle storage.
///
/// `StateMachine` is intentionally decoupled from networking and state-head ownership.
/// Callers supply the `StateHead` they want to operate against, and Postgres remains the sole
/// source of truth for which head is current.
pub struct StateMachine {
    /// RocksDB-backed app-state store used to open the created, nullifier, and
    /// state root containers during derivation.
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

    /// Parse and verify each blob as a `TxFinalized` payload, returning the
    /// valid ones in order.
    ///
    /// Fail-soft: malformed or unverifiable blobs are logged and skipped, never
    /// aborting the slot. Verification happens here once so the parsed payloads
    /// can be reused for both the existence prefetch (the caller queries the
    /// created index for their `live` commitments) and the apply pass in
    /// `derive_slot_head`. Each kept payload carries its originating blob index
    /// so the apply pass can attribute a hard error to the right blob.
    pub fn parse_blobs(
        &self,
        blob_payloads: &[(u32, Vec<u8>)],
        slot: u32,
        block_number: u32,
    ) -> Vec<(u32, Payload)> {
        let mut parsed = Vec::with_capacity(blob_payloads.len());
        for (blob_index, bytes) in blob_payloads {
            match self.proof_parser.parse_blob(bytes) {
                Ok(Some(payload)) => parsed.push((*blob_index, payload)),
                Ok(None) => info!(
                    slot,
                    block_number,
                    blob_index,
                    "Blob did not contain a valid TxFinalized proof; skipping"
                ),
                Err(err) => warn!(
                    slot,
                    block_number,
                    blob_index,
                    ?err,
                    "Failed to parse/verify TxFinalized payload; skipping blob"
                ),
            }
        }
        parsed
    }

    /// Validate one already-parsed payload against the in-progress slot state and
    /// apply it, or skip it (fail-soft) on a grounding or duplicate violation.
    ///
    /// Validation order:
    /// 1. `payload.state_root` must exist in the recent state root window
    /// 2. that grounding must be within `MAX_STATE_ROOT_AGE_BLOCKS`
    /// 3. created-object commitments must not collide -- within the payload, with
    ///    objects added earlier this slot, or with prior committed state
    ///    (`prior_indices`, prefetched from the created index and cross-checked
    ///    against the array). This is what gives no-input (mining) txs their
    ///    replay protection.
    /// 4. nullifiers must not collide within the payload or with the in-progress
    ///    nullifier set
    ///
    /// On success it appends each created commitment to the in-progress array
    /// (recording the addition in `created_added`), inserts each nullifier, and
    /// bumps the counts. Mutating the container handles may materialize Merkle
    /// nodes in RocksDB, but nothing is committed until the caller commits
    /// the resulting head.
    fn apply_payload(
        &self,
        state: &mut WorkingState,
        payload: &Payload,
        prior_indices: &HashMap<Hash, i64>,
        slot: u32,
        block_number: u32,
    ) -> Result<()> {
        let Some(&state_root_block) = state.recent_state_roots.get(&payload.state_root) else {
            warn!(
                slot,
                block_number,
                "Blob proof state_root not found in recent state root history; rejecting"
            );
            return Ok(());
        };
        let age = i64::from(block_number) - state_root_block;
        if age > MAX_STATE_ROOT_AGE_BLOCKS {
            warn!(
                slot,
                block_number, state_root_block, age, "Blob proof state_root is too old; rejecting"
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
            if self.already_created(state, prior_indices, obj)? {
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
            state.created_added.insert(*obj, index as i64);
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

    /// Whether `obj` already exists as of the in-progress slot state: it was
    /// either added earlier this slot, or the prefetched created index points at
    /// a position the array at the base root actually holds it in. Re-reading the
    /// array rejects a phantom index entry (one the root does not contain), so a
    /// stale index never wrongly rejects a legitimate creation.
    fn already_created(
        &self,
        state: &WorkingState,
        prior_indices: &HashMap<Hash, i64>,
        obj: &Hash,
    ) -> Result<bool> {
        if state.created_added.contains_key(obj) {
            return Ok(true);
        }
        match prior_indices.get(obj) {
            Some(&index) => created_array_holds(&state.created, index, *obj),
            None => Ok(false),
        }
    }

    /// Derive the next state head for one execution slot from a caller
    /// provided base head and the slot's already-parsed payloads.
    ///
    /// It:
    /// - reopens the persistent created-object, nullifiers, and state root-history containers from
    ///   `base_head.roots`
    /// - seeds the per-slot `WorkingState` with the caller-provided recent-state root window
    /// - applies every payload via `apply_payload`, using `prior_indices` (the created-object
    ///   commitments already committed as of the base head, with their array positions,
    ///   prefetched from the created index) for the creation-collision check
    /// - computes the next state root from the updated created/nullifiers roots and the prior
    ///   state root-history root committed into the resulting `StateHeader`
    /// - appends that new state root to the full history array and returns the resulting `StateHead`
    ///   together with the object commitments this slot added, as a `DerivedSlot`
    ///
    /// The returned head is only a candidate next state. By the time this method
    /// returns, RocksDB may already contain Merkle nodes for the derived containers, but the
    /// head is not committed until the caller persists it (and the `created_added`
    /// index rows) in Postgres.
    ///
    /// Empty or fully rejected slots still produce a new head with the same created and
    /// nullifiers roots as `base_head`, but with a newly derived `current_state_root` and an appended
    /// state root-history entry for the slot's execution block.
    pub fn derive_slot_head(
        &self,
        base_head: StateHead,
        recent_state_roots: impl IntoIterator<Item = (Hash, i64)>,
        slot: u32,
        block_number: u32,
        payloads: &[(u32, Payload)],
        prior_indices: &HashMap<Hash, i64>,
    ) -> Result<DerivedSlot> {
        let mut working = WorkingState {
            metadata: base_head.metadata,
            created: self.app_db.open_created(base_head.roots.created)?,
            nullifiers: self.app_db.open_nullifiers(base_head.roots.nullifiers)?,
            next_state_history: self
                .app_db
                .open_next_state_history(base_head.roots.next_state_history)?,
            recent_state_roots: recent_state_roots.into_iter().collect(),
            created_added: HashMap::new(),
        };

        for (blob_index, payload) in payloads {
            self.apply_payload(&mut working, payload, prior_indices, slot, block_number)
                .with_context(|| {
                    format!("applying blob at slot {slot}, blob_index {blob_index}")
                })?;
        }

        let prior_state_history_root = base_head.roots.next_state_history;
        let new_state_root = StateHeader::new(
            i64::from(block_number),
            working.created.commitment(),
            working.nullifiers.commitment(),
            prior_state_history_root,
        )
        .hash();

        working.next_state_history.insert(
            base_head.metadata.state_root_count as usize,
            Value::from(new_state_root),
        )?;

        let new_head = StateHead {
            roots: StateRoots {
                created: working.created.commitment(),
                nullifiers: working.nullifiers.commitment(),
                prior_state_history: prior_state_history_root,
                next_state_history: working.next_state_history.commitment(),
            },
            metadata: StateMetadata {
                current_state_root: Some(new_state_root),
                current_block_number: Some(block_number),
                created_count: working.metadata.created_count,
                nullifier_count: working.metadata.nullifier_count,
                state_root_count: base_head.metadata.state_root_count + 1,
            },
        };

        info!(
            slot,
            block_number,
            state_root_count = new_head.metadata.state_root_count,
            "Slot data"
        );

        Ok(DerivedSlot {
            head: new_head,
            created_added: working.created_added,
        })
    }

    pub fn log_current_state(&self, head: StateHead) {
        let current_state_root = head
            .metadata
            .current_state_root
            .map(|state_root| format!("{state_root:#}"))
            .unwrap_or_else(|| "none".to_string());
        info!(
            created_count = head.metadata.created_count,
            nullifier_count = head.metadata.nullifier_count,
            state_root_count = head.metadata.state_root_count,
            current_state_root = %current_state_root,
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
        state_header: Hash,
    ) -> Vec<u8> {
        let hashes_json = |hashes: &[Hash]| {
            hashes
                .iter()
                .map(|h| format!("\"{:#}\"", h))
                .collect::<Vec<_>>()
                .join(",")
        };
        format!(
            r#"{{"tx_final":"{:#}","nullifiers":[{}],"live":[{}],"state_root":"{:#}"}}"#,
            tx_final,
            hashes_json(nullifiers),
            hashes_json(live),
            state_header
        )
        .into_bytes()
    }

    fn seed_state_root0(sm: &StateMachine) -> StateHead {
        sm.derive_slot_head(StateHead::empty(), [], 0, 0, &[], &HashMap::new())
            .unwrap()
            .head
    }

    /// Parse blobs and derive one slot in one step (mirroring `Node::derive_slot`
    /// minus the Postgres existence prefetch, which tests supply directly).
    fn derive(
        sm: &StateMachine,
        base_head: StateHead,
        recent_state_roots: impl IntoIterator<Item = (Hash, i64)>,
        slot: u32,
        block_number: u32,
        blobs: &[(u32, Vec<u8>)],
        prior_indices: &HashMap<Hash, i64>,
    ) -> DerivedSlot {
        let parsed = sm.parse_blobs(blobs, slot, block_number);
        sm.derive_slot_head(
            base_head,
            recent_state_roots,
            slot,
            block_number,
            &parsed,
            prior_indices,
        )
        .unwrap()
    }

    #[test]
    fn test_empty_slot_produces_new_head() {
        let (sm, _app_db, _dir) = make_sm();
        let head = derive(&sm, StateHead::empty(), [], 1, 7, &[], &HashMap::new()).head;
        assert_eq!(head.metadata.current_block_number, Some(7));
        assert_eq!(head.metadata.state_root_count, 1);
    }

    #[test]
    fn test_accepts_valid_blob_and_updates_counts() {
        let (sm, app_db, _dir) = make_sm();
        let head0 = seed_state_root0(&sm);
        let state_root0 = head0.metadata.current_state_root.unwrap();

        let tx_final = unique_hash(10);
        let nullifier = unique_hash(11);
        let live_obj = unique_hash(12);
        let blob = mock_txn_bytes(tx_final, &[nullifier], &[live_obj], state_root0);
        let derived = derive(
            &sm,
            head0,
            [(state_root0, 0)],
            1,
            1,
            &[(0, blob)],
            &HashMap::new(),
        );
        let head1 = derived.head;
        let created_present = app_db
            .created_exists_for(&head1.roots, &[live_obj], &derived.created_added)
            .unwrap();
        let nullifier_present = app_db
            .nullifier_exists_batch(&head1.roots, &[nullifier])
            .unwrap();

        assert_eq!(head1.metadata.created_count, 1);
        assert_eq!(head1.metadata.nullifier_count, 1);
        assert_eq!(head1.metadata.state_root_count, 2);
        assert_eq!(created_present, vec![true]);
        assert_eq!(nullifier_present, vec![true]);
    }

    #[test]
    fn test_rejects_unknown_grounding_state_root() {
        let (sm, _app_db, _dir) = make_sm();
        let head0 = seed_state_root0(&sm);

        let tx_final = unique_hash(21);
        let blob = mock_txn_bytes(tx_final, &[], &[unique_hash(22)], unique_hash(99));
        let head1 = derive(&sm, head0, [], 2, 2, &[(0, blob)], &HashMap::new()).head;
        assert_eq!(head1.metadata.created_count, head0.metadata.created_count);
    }

    #[test]
    fn test_membership_is_scoped_to_head_root() {
        let (sm, app_db, _dir) = make_sm();
        let head0 = seed_state_root0(&sm);
        let state_root0 = head0.metadata.current_state_root.unwrap();

        let live_obj = unique_hash(32);
        let blob = mock_txn_bytes(unique_hash(31), &[], &[live_obj], state_root0);
        let derived = derive(
            &sm,
            head0,
            [(state_root0, 0)],
            1,
            1,
            &[(0, blob)],
            &HashMap::new(),
        );
        let head1 = derived.head;
        let indices = derived.created_added;

        // The same index cross-checks against each head's root: present under
        // the new head, absent under the old.
        let old_membership = app_db
            .created_exists_for(&head0.roots, &[live_obj], &indices)
            .unwrap();
        let new_membership = app_db
            .created_exists_for(&head1.roots, &[live_obj], &indices)
            .unwrap();

        assert_eq!(old_membership, vec![false]);
        assert_eq!(new_membership, vec![true]);
    }

    #[test]
    fn test_rejects_duplicate_created_object() {
        let (sm, _app_db, _dir) = make_sm();
        let head0 = seed_state_root0(&sm);
        let state_root0 = head0.metadata.current_state_root.unwrap();

        let live_obj = unique_hash(40);
        let blob1 = mock_txn_bytes(unique_hash(41), &[], &[live_obj], state_root0);
        let derived1 = derive(
            &sm,
            head0,
            [(state_root0, 0)],
            1,
            1,
            &[(0, blob1)],
            &HashMap::new(),
        );
        let head1 = derived1.head;
        assert_eq!(head1.metadata.created_count, 1);

        // A second tx in a later slot re-creates the same object. In production
        // slot 1's index row is committed; here it is passed as prior-existing.
        let state_root1 = head1.metadata.current_state_root.unwrap();
        let blob2 = mock_txn_bytes(unique_hash(42), &[], &[live_obj], state_root1);
        let head2 = derive(
            &sm,
            head1,
            [(state_root1, 1)],
            2,
            2,
            &[(0, blob2)],
            &derived1.created_added,
        )
        .head;
        assert_eq!(head2.metadata.created_count, head1.metadata.created_count);
    }

    #[test]
    fn test_rejects_duplicate_created_object_within_slot() {
        let (sm, _app_db, _dir) = make_sm();
        let head0 = seed_state_root0(&sm);
        let state_root0 = head0.metadata.current_state_root.unwrap();

        // Two blobs in one slot create the same object; the second is rejected
        // via the in-slot view, with no prior committed state.
        let dup = unique_hash(50);
        let blob_a = mock_txn_bytes(unique_hash(51), &[], &[dup], state_root0);
        let blob_b = mock_txn_bytes(unique_hash(52), &[], &[dup], state_root0);
        let derived = derive(
            &sm,
            head0,
            [(state_root0, 0)],
            1,
            1,
            &[(0, blob_a), (1, blob_b)],
            &HashMap::new(),
        );
        assert_eq!(derived.head.metadata.created_count, 1);
        assert_eq!(derived.created_added.get(&dup), Some(&0));
    }

    #[test]
    fn test_phantom_prior_index_does_not_reject_creation() {
        let (sm, _app_db, _dir) = make_sm();
        let head0 = seed_state_root0(&sm);
        let state_root0 = head0.metadata.current_state_root.unwrap();

        // The prefetched index claims this object exists at index 1, but the
        // array at the base root does not actually hold it (a phantom/stale
        // entry). The array cross-check must treat it as absent and accept the
        // creation rather than rejecting a legitimate object.
        let obj = unique_hash(60);
        let phantom = HashMap::from([(obj, 1i64)]);
        let blob = mock_txn_bytes(unique_hash(61), &[], &[obj], state_root0);
        let derived = derive(&sm, head0, [(state_root0, 0)], 1, 1, &[(0, blob)], &phantom);
        assert_eq!(derived.head.metadata.created_count, 1);
        assert_eq!(derived.created_added.get(&obj), Some(&0));
    }
}
