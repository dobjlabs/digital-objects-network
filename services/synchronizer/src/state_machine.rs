use anyhow::{Context, Result};
use payload::{payload::Payload, proof::BlobParser};
use pod2::middleware::{containers::Array, containers::Set, Hash, Value};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use tracing::{info, warn};

use crate::{
    container_db::{created_array_holds, ContainerDb},
    don_state::{DonState, ExecutionBlock, StateMetadata, StateRoots},
};
use pod2utils::b256_to_hash;
use txlib::StateHeader;

/// The maximum age of a state root used as grounding for a transaction.
/// At one block per 12 seconds, this is one hour.
pub const MAX_STATE_ROOT_AGE_BLOCKS: i64 = 300;

/// The base state's containers and counts, mutated while deriving one slot.
struct WorkingState {
    /// Objects in the created set, counting those this slot appended so far.
    created_count: u64,
    /// Spent nullifiers, counting those this slot inserted so far.
    nullifier_count: u64,
    created: Array,
    nullifiers: Set,
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

/// One slot's derivation result: the candidate next state plus the
/// object commitments (with array indices) it appended to `created`. The caller
/// persists the additions to the created index when committing the slot.
pub struct DerivedSlot {
    pub state: DonState,
    pub created_added: HashMap<Hash, i64>,
}

/// Domain logic for the synchronizer: proof verification, state validation, and Merkle storage.
/// Callers supply the base state; which state is the head is decided by Postgres.
pub struct StateMachine {
    /// RocksDB store of the Merkle containers, used to open the created, nullifier, and
    /// state root containers during derivation.
    container_db: ContainerDb,
    /// Blob parser/verifier used to decode TxFinalized payloads from blob bytes.
    proof_parser: Arc<dyn BlobParser>,
}

impl StateMachine {
    pub fn new(container_db: ContainerDb, proof_parser: Arc<dyn BlobParser>) -> Self {
        Self {
            container_db,
            proof_parser,
        }
    }

    /// Sync DB to disk
    pub fn sync(&self) -> Result<()> {
        self.container_db.sync()
    }

    /// Parse and verify each blob as a `TxFinalized` payload, returning the
    /// valid ones in order.
    ///
    /// Fail-soft: malformed or unverifiable blobs are logged and skipped, never
    /// aborting the slot. Verification happens here once so the parsed payloads
    /// can be reused for both the existence prefetch (the caller queries the
    /// created index for their `created` commitments) and the apply pass in
    /// `derive_slot_state`. Each kept payload carries its originating blob index
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
    /// the resulting state.
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
        let mut payload_created = HashSet::with_capacity(payload.created.len());
        for obj in &payload.created {
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

        for obj in &payload.created {
            // The created array is 0-indexed: the next object lands at slot
            // `created_count`, which doubles as the true object count.
            let index = state.created_count as usize;
            state.created.insert(index, Value::from(*obj))?;
            state.created_added.insert(*obj, index as i64);
            state.created_count += 1;
        }
        for nullifier in &payload.nullifiers {
            state.nullifiers.insert(&Value::from(*nullifier))?;
            state.nullifier_count += 1;
        }

        info!(
            slot,
            block_number,
            created_count = state.created_count,
            nullifier_count = state.nullifier_count,
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

    /// Derive the state at `block` from `base_state` (`None` derives the genesis state) by
    /// applying the slot's parsed payloads and appending the new state root to the history.
    /// `prior_indices` are the created-index positions of the payloads' created commitments,
    /// for the creation-collision check.
    ///
    /// The result is only a candidate: RocksDB may already hold its Merkle nodes, but it is not
    /// committed until the caller persists it, with its `created_added` rows, in Postgres.
    pub fn derive_slot_state(
        &self,
        base_state: Option<DonState>,
        recent_state_roots: impl IntoIterator<Item = (Hash, i64)>,
        slot: u32,
        block: ExecutionBlock,
        payloads: &[(u32, Payload)],
        prior_indices: &HashMap<Hash, i64>,
    ) -> Result<DerivedSlot> {
        let base_roots = base_state.map_or(StateRoots::empty(), |state| state.roots);
        let base_state_root_count = base_state.map_or(0, |state| state.metadata.state_root_count);
        let mut working = WorkingState {
            created_count: base_state.map_or(0, |state| state.metadata.created_count),
            nullifier_count: base_state.map_or(0, |state| state.metadata.nullifier_count),
            created: self.container_db.open_created(base_roots.created)?,
            nullifiers: self.container_db.open_nullifiers(base_roots.nullifiers)?,
            next_state_history: self
                .container_db
                .open_next_state_history(base_roots.next_state_history)?,
            recent_state_roots: recent_state_roots.into_iter().collect(),
            created_added: HashMap::new(),
        };

        for (blob_index, payload) in payloads {
            self.apply_payload(&mut working, payload, prior_indices, slot, block.number)
                .with_context(|| {
                    format!("applying blob at slot {slot}, blob_index {blob_index}")
                })?;
        }

        let prior_state_history_root = base_roots.next_state_history;
        let new_state_root = StateHeader::new(
            i64::from(block.number),
            block.timestamp as i64,
            b256_to_hash(block.hash),
            working.created.commitment(),
            working.nullifiers.commitment(),
            prior_state_history_root,
        )
        .hash();

        working
            .next_state_history
            .insert(base_state_root_count as usize, Value::from(new_state_root))?;

        let new_state = DonState {
            roots: StateRoots {
                created: working.created.commitment(),
                nullifiers: working.nullifiers.commitment(),
                prior_state_history: prior_state_history_root,
                next_state_history: working.next_state_history.commitment(),
            },
            metadata: StateMetadata {
                state_root: new_state_root,
                block,
                created_count: working.created_count,
                nullifier_count: working.nullifier_count,
                state_root_count: base_state_root_count + 1,
            },
        };

        info!(
            slot,
            block_number = block.number,
            state_root_count = new_state.metadata.state_root_count,
            "Slot data"
        );

        Ok(DerivedSlot {
            state: new_state,
            created_added: working.created_added,
        })
    }

    pub fn log_state(&self, state: DonState) {
        info!(
            created_count = state.metadata.created_count,
            nullifier_count = state.metadata.nullifier_count,
            state_root_count = state.metadata.state_root_count,
            state_root = %format!("{:#}", state.metadata.state_root),
            "State"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::container_db::ContainerDb;
    use payload::proof::MockBlobParser;
    use pod2::middleware::hash_values;
    use tempfile::TempDir;

    fn make_sm() -> (StateMachine, ContainerDb, TempDir) {
        let dir = TempDir::new().unwrap();
        let container_db = ContainerDb::connect(dir.path().to_str().unwrap()).unwrap();
        let sm = StateMachine::new(container_db.clone(), Arc::new(MockBlobParser));
        (sm, container_db, dir)
    }

    fn unique_hash(n: i64) -> Hash {
        hash_values(&[Value::from(n)])
    }

    fn block_meta(n: u32) -> ExecutionBlock {
        ExecutionBlock {
            number: n,
            timestamp: (n as u64) * 1000,
            hash: alloy::primitives::B256::repeat_byte(n as u8),
        }
    }

    fn mock_txn_bytes(
        tx_final: Hash,
        nullifiers: &[Hash],
        created: &[Hash],
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
            r#"{{"tx_final":"{:#}","nullifiers":[{}],"created":[{}],"state_root":"{:#}"}}"#,
            tx_final,
            hashes_json(nullifiers),
            hashes_json(created),
            state_header
        )
        .into_bytes()
    }

    fn seed_state_root0(sm: &StateMachine) -> DonState {
        sm.derive_slot_state(None, [], 0, block_meta(0), &[], &HashMap::new())
            .unwrap()
            .state
    }

    /// Parse blobs and derive one slot in one step (mirroring `Node::derive_slot`
    /// minus the Postgres existence prefetch, which tests supply directly).
    fn derive(
        sm: &StateMachine,
        base_state: Option<DonState>,
        recent_state_roots: impl IntoIterator<Item = (Hash, i64)>,
        slot: u32,
        block_meta: ExecutionBlock,
        blobs: &[(u32, Vec<u8>)],
        prior_indices: &HashMap<Hash, i64>,
    ) -> DerivedSlot {
        let parsed = sm.parse_blobs(blobs, slot, block_meta.number);
        sm.derive_slot_state(
            base_state,
            recent_state_roots,
            slot,
            block_meta,
            &parsed,
            prior_indices,
        )
        .unwrap()
    }

    #[test]
    fn test_empty_slot_produces_new_state() {
        let (sm, _container_db, _dir) = make_sm();
        let state = derive(&sm, None, [], 1, block_meta(7), &[], &HashMap::new()).state;
        assert_eq!(state.metadata.block.number, 7);
        assert_eq!(state.metadata.state_root_count, 1);
    }

    #[test]
    fn test_accepts_valid_blob_and_updates_counts() {
        let (sm, container_db, _dir) = make_sm();
        let state0 = seed_state_root0(&sm);
        let state_root0 = state0.metadata.state_root;

        let tx_final = unique_hash(10);
        let nullifier = unique_hash(11);
        let created_obj = unique_hash(12);
        let blob = mock_txn_bytes(tx_final, &[nullifier], &[created_obj], state_root0);
        let derived = derive(
            &sm,
            Some(state0),
            [(state_root0, 0)],
            1,
            block_meta(1),
            &[(0, blob)],
            &HashMap::new(),
        );
        let state1 = derived.state;
        let created_present = container_db
            .created_exists_for(&state1.roots, &[created_obj], &derived.created_added)
            .unwrap();
        let nullifier_present = container_db
            .nullifier_exists_batch(&state1.roots, &[nullifier])
            .unwrap();

        assert_eq!(state1.metadata.created_count, 1);
        assert_eq!(state1.metadata.nullifier_count, 1);
        assert_eq!(state1.metadata.state_root_count, 2);
        assert_eq!(created_present, vec![true]);
        assert_eq!(nullifier_present, vec![true]);
    }

    #[test]
    fn records_intermediate_states_alongside_their_nullifiers() {
        use pod2::middleware::{containers::Dictionary, StrKey};
        let (sm, container_db, _dir) = make_sm();
        let state0 = seed_state_root0(&sm);
        let state_root0 = state0.metadata.state_root;
        let intermediate = Dictionary::new(HashMap::from([
            (StrKey::from("key"), Value::from(1_i64)),
            (StrKey::from("version"), Value::from(0_i64)),
        ]));
        let mut final_state = intermediate.clone();
        final_state
            .update(&StrKey::from("version"), &Value::from(1_i64))
            .unwrap();
        let created = [intermediate.commitment(), final_state.commitment()];
        let nullifier = txlib::object_nullifier_hash(&intermediate).unwrap();
        let blob = mock_txn_bytes(unique_hash(70), &[nullifier], &created, state_root0);
        let derived = derive(
            &sm,
            Some(state0),
            [(state_root0, 0)],
            1,
            block_meta(1),
            &[(0, blob)],
            &HashMap::new(),
        );
        assert_eq!(derived.state.metadata.created_count, 2);
        assert_eq!(derived.state.metadata.nullifier_count, 1);
        assert_eq!(
            container_db
                .created_exists_for(&derived.state.roots, &created, &derived.created_added)
                .unwrap(),
            vec![true, true]
        );
        assert_eq!(
            container_db
                .nullifier_exists_batch(&derived.state.roots, &[nullifier])
                .unwrap(),
            vec![true]
        );

        let state_root1 = derived.state.metadata.state_root;
        let replay = mock_txn_bytes(
            unique_hash(71),
            &[],
            &[intermediate.commitment()],
            state_root1,
        );
        let state2 = derive(
            &sm,
            Some(derived.state),
            [(state_root1, 1)],
            2,
            block_meta(2),
            &[(0, replay)],
            &derived.created_added,
        )
        .state;
        assert_eq!(
            state2.metadata.created_count, 2,
            "spent intermediate states must still reject creation collisions"
        );
    }

    #[test]
    fn test_rejects_unknown_grounding_state_root() {
        let (sm, _container_db, _dir) = make_sm();
        let state0 = seed_state_root0(&sm);

        let tx_final = unique_hash(21);
        let blob = mock_txn_bytes(tx_final, &[], &[unique_hash(22)], unique_hash(99));
        let state1 = derive(
            &sm,
            Some(state0),
            [],
            2,
            block_meta(2),
            &[(0, blob)],
            &HashMap::new(),
        )
        .state;
        assert_eq!(state1.metadata.created_count, state0.metadata.created_count);
    }

    #[test]
    fn test_membership_is_scoped_to_state_roots() {
        let (sm, container_db, _dir) = make_sm();
        let state0 = seed_state_root0(&sm);
        let state_root0 = state0.metadata.state_root;

        let created_obj = unique_hash(32);
        let blob = mock_txn_bytes(unique_hash(31), &[], &[created_obj], state_root0);
        let derived = derive(
            &sm,
            Some(state0),
            [(state_root0, 0)],
            1,
            block_meta(0),
            &[(0, blob)],
            &HashMap::new(),
        );
        let state1 = derived.state;
        let indices = derived.created_added;

        // The same index cross-checks against each state's root: present under
        // the new state, absent under the old.
        let old_membership = container_db
            .created_exists_for(&state0.roots, &[created_obj], &indices)
            .unwrap();
        let new_membership = container_db
            .created_exists_for(&state1.roots, &[created_obj], &indices)
            .unwrap();

        assert_eq!(old_membership, vec![false]);
        assert_eq!(new_membership, vec![true]);
    }

    #[test]
    fn test_rejects_duplicate_created_object() {
        let (sm, _container_db, _dir) = make_sm();
        let state0 = seed_state_root0(&sm);
        let state_root0 = state0.metadata.state_root;

        let created_obj = unique_hash(40);
        let blob1 = mock_txn_bytes(unique_hash(41), &[], &[created_obj], state_root0);
        let derived1 = derive(
            &sm,
            Some(state0),
            [(state_root0, 0)],
            1,
            block_meta(1),
            &[(0, blob1)],
            &HashMap::new(),
        );
        let state1 = derived1.state;
        assert_eq!(state1.metadata.created_count, 1);

        // A second tx in a later slot re-creates the same object. In production
        // slot 1's index row is committed; here it is passed as prior-existing.
        let state_root1 = state1.metadata.state_root;
        let blob2 = mock_txn_bytes(unique_hash(42), &[], &[created_obj], state_root1);
        let state2 = derive(
            &sm,
            Some(state1),
            [(state_root1, 1)],
            2,
            block_meta(2),
            &[(0, blob2)],
            &derived1.created_added,
        )
        .state;
        assert_eq!(state2.metadata.created_count, state1.metadata.created_count);
    }

    #[test]
    fn test_rejects_duplicate_created_object_within_slot() {
        let (sm, _container_db, _dir) = make_sm();
        let state0 = seed_state_root0(&sm);
        let state_root0 = state0.metadata.state_root;

        // Two blobs in one slot create the same object; the second is rejected
        // via the in-slot view, with no prior committed state.
        let dup = unique_hash(50);
        let blob_a = mock_txn_bytes(unique_hash(51), &[], &[dup], state_root0);
        let blob_b = mock_txn_bytes(unique_hash(52), &[], &[dup], state_root0);
        let derived = derive(
            &sm,
            Some(state0),
            [(state_root0, 0)],
            1,
            block_meta(1),
            &[(0, blob_a), (1, blob_b)],
            &HashMap::new(),
        );
        assert_eq!(derived.state.metadata.created_count, 1);
        assert_eq!(derived.created_added.get(&dup), Some(&0));
    }

    #[test]
    fn test_phantom_prior_index_does_not_reject_creation() {
        let (sm, _container_db, _dir) = make_sm();
        let state0 = seed_state_root0(&sm);
        let state_root0 = state0.metadata.state_root;

        // The prefetched index claims this object exists at index 1, but the
        // array at the base root does not actually hold it (a phantom/stale
        // entry). The array cross-check must treat it as absent and accept the
        // creation rather than rejecting a legitimate object.
        let obj = unique_hash(60);
        let phantom = HashMap::from([(obj, 1i64)]);
        let blob = mock_txn_bytes(unique_hash(61), &[], &[obj], state_root0);
        let derived = derive(
            &sm,
            Some(state0),
            [(state_root0, 0)],
            1,
            block_meta(1),
            &[(0, blob)],
            &phantom,
        );
        assert_eq!(derived.state.metadata.created_count, 1);
        assert_eq!(derived.created_added.get(&obj), Some(&0));
    }
}
