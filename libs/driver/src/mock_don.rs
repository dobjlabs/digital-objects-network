//! In-process stand-in for the DON network, used in mock mode. One shared state serves both
//! the synchronizer and the relayer: a submitted transaction is checked with the
//! synchronizer's rules and applied at once, in a block of its own. Nothing is persisted.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use anyhow::{Result, anyhow};
use payload::encode_hash_hex;
use pod2::middleware::{
    F, Hash, Value,
    containers::{Array, Set},
};
use sdk::SpendableObjects;
use serde::{Deserialize, Serialize};
use txlib::{GroundingWitness, StateHeader, grounding_aged_out};
use wire_types::relayer::{JobStatus, JobStatusResponse, SubmitProofResponse};

use crate::clients::{
    RelayerClient, SynchronizerClient, SynchronizerHead, SynchronizerMembership,
    SynchronizerTxStatus,
};
use crate::driver::PayloadBuilder;

/// Set to anything but empty or `0` to run the driver against a [`MockDon`] with mock proofs.
pub const DOBJ_MOCK_ENV: &str = "DOBJ_MOCK";

pub(crate) fn mock_mode_from_env() -> bool {
    std::env::var(DOBJ_MOCK_ENV).is_ok_and(|value| !value.is_empty() && value != "0")
}

/// What the mock relayer receives instead of a proof-bearing blob payload.
#[derive(Serialize, Deserialize)]
struct MockPayload {
    tx_final: Hash,
    state_root: Hash,
    nullifiers: Vec<Hash>,
    created: Vec<Hash>,
}

/// Builds a [`MockPayload`], skipping the proof shrinking that mock pods cannot go through.
#[derive(Clone, Default)]
pub(crate) struct MockPayloadBuilder;

impl PayloadBuilder for MockPayloadBuilder {
    fn build_payload(
        &self,
        old_state_root: &Hash,
        action_output: &SpendableObjects,
    ) -> Result<Vec<u8>> {
        let tx = &action_output.tx;
        let payload = MockPayload {
            tx_final: tx.dict().commitment(),
            state_root: *old_state_root,
            nullifiers: tx.nullifier_hashes()?,
            created: tx.created_commitments()?,
        };
        Ok(serde_json::to_vec(&payload)?)
    }
}

struct MockJob {
    tx_final: Hash,
    state_root: Hash,
    tx_hash: String,
    block_number: i64,
    created_at: i64,
}

struct MockDonState {
    head: StateHeader,
    created: Array,
    created_index: HashMap<Hash, i64>,
    nullifiers: Set,
    nullifier_hashes: HashSet<Hash>,
    /// Every state root except the head's, in block order.
    state_history: Array,
    /// Block number of every state root, the head's included.
    root_blocks: HashMap<Hash, i64>,
    tx_statuses: HashMap<Hash, SynchronizerTxStatus>,
    jobs: HashMap<String, MockJob>,
    job_ids_by_tx_final: HashMap<Hash, String>,
}

fn block_hash(block_number: i64) -> Hash {
    Hash([F(0), F(0), F(0), F(block_number as u64)])
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_secs() as i64
}

impl MockDonState {
    fn genesis() -> Self {
        let created = Array::new(Vec::new());
        let nullifiers = Set::new(HashSet::new());
        let state_history = Array::new(Vec::new());
        let head = StateHeader::new(
            1,
            now_secs(),
            block_hash(1),
            created.commitment(),
            nullifiers.commitment(),
            state_history.commitment(),
        );
        let root_blocks = HashMap::from([(head.hash(), head.block_number)]);
        Self {
            head,
            created,
            created_index: HashMap::new(),
            nullifiers,
            nullifier_hashes: HashSet::new(),
            state_history,
            root_blocks,
            tx_statuses: HashMap::new(),
            jobs: HashMap::new(),
            job_ids_by_tx_final: HashMap::new(),
        }
    }

    /// Why the synchronizer would reject `payload` in the next block, if it would.
    fn rejection(&self, payload: &MockPayload) -> Option<String> {
        let inclusion_block = self.head.block_number + 1;
        let Some(&root_block) = self.root_blocks.get(&payload.state_root) else {
            return Some("Blob proof state_root not found in recent state root history".into());
        };
        if grounding_aged_out(inclusion_block, root_block) {
            let age = inclusion_block - root_block;
            return Some(format!(
                "Blob proof state_root is too old: {age} blocks (state root at block {root_block})"
            ));
        }
        let mut payload_created = HashSet::new();
        for commitment in &payload.created {
            if !payload_created.insert(commitment) {
                return Some("Duplicate created object within payload".into());
            }
            if self.created_index.contains_key(commitment) {
                return Some("Created object already exists (creation collision)".into());
            }
        }
        let mut payload_nullifiers = HashSet::new();
        for nullifier in &payload.nullifiers {
            if !payload_nullifiers.insert(nullifier) {
                return Some("Duplicate nullifier within payload".into());
            }
            if self.nullifier_hashes.contains(nullifier) {
                return Some("Duplicate nullifier".into());
            }
        }
        None
    }

    fn apply(&mut self, payload: &MockPayload) -> Result<()> {
        for commitment in &payload.created {
            let index = self.created_index.len() as i64;
            self.created
                .insert(index as usize, Value::from(*commitment))?;
            self.created_index.insert(*commitment, index);
        }
        for nullifier in &payload.nullifiers {
            self.nullifiers.insert(&Value::from(*nullifier))?;
            self.nullifier_hashes.insert(*nullifier);
        }
        let history_len = self.root_blocks.len() - 1;
        self.state_history
            .insert(history_len, Value::from(self.head.hash()))?;
        let block_number = self.head.block_number + 1;
        self.head = StateHeader::new(
            block_number,
            now_secs().max(self.head.block_timestamp),
            block_hash(block_number),
            self.created.commitment(),
            self.nullifiers.commitment(),
            self.state_history.commitment(),
        );
        self.root_blocks.insert(self.head.hash(), block_number);
        Ok(())
    }

    fn submit(&mut self, payload_bytes: &[u8]) -> Result<SubmitProofResponse> {
        let payload: MockPayload = serde_json::from_slice(payload_bytes)
            .map_err(|err| anyhow!("invalid mock payload: {err}"))?;
        if let Some(job_id) = self.job_ids_by_tx_final.get(&payload.tx_final) {
            return Ok(self.submit_response(job_id));
        }

        let error = self.rejection(&payload);
        if error.is_none() {
            self.apply(&payload)?;
        }
        let job_id = format!("mock-{}", self.jobs.len() + 1);
        let tx_hash = encode_hash_hex(&payload.tx_final);
        let block_number = self.head.block_number;
        self.tx_statuses.insert(
            payload.tx_final,
            SynchronizerTxStatus {
                tx_hash: tx_hash.clone(),
                block_number,
                error,
            },
        );
        self.jobs.insert(
            job_id.clone(),
            MockJob {
                tx_final: payload.tx_final,
                state_root: payload.state_root,
                tx_hash,
                block_number,
                created_at: now_secs(),
            },
        );
        self.job_ids_by_tx_final
            .insert(payload.tx_final, job_id.clone());
        Ok(self.submit_response(&job_id))
    }

    fn submit_response(&self, job_id: &str) -> SubmitProofResponse {
        let job = &self.jobs[job_id];
        SubmitProofResponse {
            job_id: job_id.to_string(),
            status: JobStatus::Confirmed,
            tx_final: job.tx_final,
            state_root: job.state_root,
            attempt_count: 1,
            created_at: job.created_at,
        }
    }

    fn grounding_witness(&self, object_commitments: &[Hash]) -> Result<GroundingWitness> {
        let missing: Vec<String> = object_commitments
            .iter()
            .filter(|commitment| !self.created_index.contains_key(commitment))
            .map(|commitment| format!("{commitment:#}"))
            .collect();
        if !missing.is_empty() {
            return Err(anyhow!(
                "input not yet synchronized; wait and retry: {}",
                missing.join(", ")
            ));
        }
        let mut created_proofs = HashMap::new();
        for commitment in object_commitments {
            let index = self.created_index[commitment];
            let (_, proof) = self.created.prove(index as usize)?;
            created_proofs.insert(*commitment, (index, proof));
        }
        Ok(GroundingWitness::new(self.head.clone(), created_proofs))
    }
}

/// Serves the synchronizer and relayer APIs from one in-memory DON state, ignoring the
/// configured URLs.
#[derive(Clone)]
pub(crate) struct MockDon {
    state: Arc<Mutex<MockDonState>>,
}

impl MockDon {
    pub(crate) fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(MockDonState::genesis())),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, MockDonState> {
        self.state.lock().expect("mock DON lock poisoned")
    }
}

impl SynchronizerClient for MockDon {
    fn fetch_head(&self, _sync_api_url: &str) -> Result<SynchronizerHead> {
        let state = self.state();
        Ok(SynchronizerHead {
            state_root: state.head.hash(),
            block_number: state.head.block_number,
        })
    }

    fn fetch_tx_status(
        &self,
        _sync_api_url: &str,
        tx_final: &Hash,
    ) -> Result<Option<SynchronizerTxStatus>> {
        Ok(self.state().tx_statuses.get(tx_final).cloned())
    }

    fn fetch_grounding_witness(
        &self,
        _sync_api_url: &str,
        object_commitments: &[Hash],
    ) -> Result<GroundingWitness> {
        self.state().grounding_witness(object_commitments)
    }

    fn fetch_membership_with_nullifiers(
        &self,
        _sync_api_url: &str,
        object_commitments: &[Hash],
        nullifiers: &[Hash],
    ) -> Result<SynchronizerMembership> {
        let state = self.state();
        Ok(SynchronizerMembership {
            created_objects: object_commitments
                .iter()
                .filter(|commitment| state.created_index.contains_key(commitment))
                .copied()
                .collect(),
            on_chain_nullifiers: nullifiers
                .iter()
                .filter(|nullifier| state.nullifier_hashes.contains(nullifier))
                .copied()
                .collect(),
            block_number: state.head.block_number,
        })
    }
}

impl RelayerClient for MockDon {
    fn submit_proof(
        &self,
        _relayer_api_url: &str,
        payload_bytes: &[u8],
        _client_ref: Option<String>,
    ) -> Result<SubmitProofResponse> {
        self.state().submit(payload_bytes)
    }

    fn fetch_job_status(&self, _relayer_api_url: &str, job_id: &str) -> Result<JobStatusResponse> {
        let state = self.state();
        let job = state
            .jobs
            .get(job_id)
            .ok_or_else(|| anyhow!("mock relayer job {job_id} not found"))?;
        Ok(JobStatusResponse {
            job_id: job_id.to_string(),
            status: JobStatus::Confirmed,
            tx_hash: Some(job.tx_hash.clone()),
            block_number: Some(job.block_number as u64),
            attempt_count: 1,
            last_error: None,
            updated_at: job.created_at,
            created_at: job.created_at,
            tx_final: job.tx_final,
            state_root: job.state_root,
        })
    }
}

#[cfg(test)]
mod tests {
    use txlib::MAX_STATE_ROOT_AGE_BLOCKS;

    use super::*;

    fn test_hash(seed: u64) -> Hash {
        Hash([F(seed), F(1), F(2), F(3)])
    }

    fn payload_bytes(
        state_root: Hash,
        tx_seed: u64,
        created: &[Hash],
        nullifiers: &[Hash],
    ) -> Vec<u8> {
        serde_json::to_vec(&MockPayload {
            tx_final: test_hash(1000 + tx_seed),
            state_root,
            nullifiers: nullifiers.to_vec(),
            created: created.to_vec(),
        })
        .unwrap()
    }

    #[test]
    fn test_applied_tx_grounds_the_next_one() {
        let don = MockDon::new();
        let genesis = don.fetch_head("").unwrap();
        let first = test_hash(1);
        let response = don
            .submit_proof(
                "",
                &payload_bytes(genesis.state_root, 1, &[first], &[]),
                None,
            )
            .unwrap();
        let status = don
            .fetch_tx_status("", &response.tx_final)
            .unwrap()
            .unwrap();
        assert_eq!(status.error, None);

        let head = don.fetch_head("").unwrap();
        assert_eq!(head.block_number, genesis.block_number + 1);
        let witness = don.fetch_grounding_witness("", &[first]).unwrap();
        assert_eq!(witness.state_header.hash(), head.state_root);
        let (index, proof) = &witness.created_proofs[&first];
        Array::verify(
            witness.state_header.created_root,
            proof,
            *index as usize,
            &Value::from(first),
        )
        .unwrap();
        assert_ne!(
            witness.state_header.prior_state_history_root,
            Array::new(Vec::new()).commitment()
        );

        assert!(don.fetch_grounding_witness("", &[test_hash(2)]).is_err());
    }

    #[test]
    fn test_double_spend_is_rejected() {
        let don = MockDon::new();
        let root = don.fetch_head("").unwrap().state_root;
        let nullifier = test_hash(7);
        don.submit_proof(
            "",
            &payload_bytes(root, 1, &[test_hash(1)], &[nullifier]),
            None,
        )
        .unwrap();
        let second = don
            .submit_proof(
                "",
                &payload_bytes(root, 2, &[test_hash(2)], &[nullifier]),
                None,
            )
            .unwrap();
        let status = don.fetch_tx_status("", &second.tx_final).unwrap().unwrap();
        assert_eq!(status.error.as_deref(), Some("Duplicate nullifier"));
        let membership = don
            .fetch_membership_with_nullifiers("", &[test_hash(1), test_hash(2)], &[nullifier])
            .unwrap();
        assert_eq!(membership.created_objects, HashSet::from([test_hash(1)]));
    }

    #[test]
    fn test_resubmission_is_idempotent() {
        let don = MockDon::new();
        let root = don.fetch_head("").unwrap().state_root;
        let bytes = payload_bytes(root, 1, &[test_hash(1)], &[]);
        let first = don.submit_proof("", &bytes, None).unwrap();
        let second = don.submit_proof("", &bytes, None).unwrap();
        assert_eq!(first.job_id, second.job_id);
        assert_eq!(don.fetch_head("").unwrap().block_number, 2);
    }

    #[test]
    fn test_aged_out_root_is_rejected() {
        let don = MockDon::new();
        let genesis_root = don.fetch_head("").unwrap().state_root;
        for seed in 0..MAX_STATE_ROOT_AGE_BLOCKS as u64 {
            let root = don.fetch_head("").unwrap().state_root;
            don.submit_proof(
                "",
                &payload_bytes(root, seed, &[test_hash(seed)], &[]),
                None,
            )
            .unwrap();
        }
        let late = don
            .submit_proof(
                "",
                &payload_bytes(genesis_root, 9999, &[test_hash(9999)], &[]),
                None,
            )
            .unwrap();
        let status = don.fetch_tx_status("", &late.tx_final).unwrap().unwrap();
        assert!(
            status
                .error
                .unwrap()
                .starts_with("Blob proof state_root is too old")
        );
    }
}
