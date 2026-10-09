use std::{
    collections::HashMap,
    env,
    time::{Duration, Instant},
};

use base64::{Engine, engine::general_purpose::STANDARD};
use pod2::middleware::Hash;
use reqwest::blocking::Client;
use txlib::{GroundingWitness, StateHeader};
use wire_types::{
    relayer::{JobStatus, JobStatusResponse, SubmitProofRequest, SubmitProofResponse},
    synchronizer::{
        GroundingWitnessRequest, GroundingWitnessResponse, MembershipRequest, MembershipResponse,
    },
};

use crate::PreparedTx;

const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// How often unlanded transactions are checked for a failed relayer job, so a rejected proof
/// stops the run instead of waiting out `LANDING_TIMEOUT`.
const RELAYER_CHECK_INTERVAL: Duration = Duration::from_secs(15);
const LANDING_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// The synchronizer rejects membership queries with more hashes than this.
const MEMBERSHIP_QUERY_LIMIT: usize = 256;

/// A transaction handed to the relayer, waiting to show up in the synchronizer's state.
pub struct SubmittedTx {
    pub job_id: String,
    pub created: Vec<Hash>,
    pub nullifiers: Vec<Hash>,
    /// Measured from the timer passed to [`Services::wait_for_landing`].
    pub landed_after: Option<Duration>,
}

/// The relayer and synchronizer under test.
pub struct Services {
    http: Client,
    relayer_url: String,
    synchronizer_url: String,
    /// Tags relayer jobs with the binary that submitted them.
    client_ref: String,
}

impl Services {
    /// Reads `RELAYER_URL` and `SYNCHRONIZER_URL` from this crate's `.env`.
    pub fn from_env(client_ref: &str) -> Self {
        dotenvy::from_path(concat!(env!("CARGO_MANIFEST_DIR"), "/.env")).unwrap();
        Self {
            http: Client::new(),
            relayer_url: env::var("RELAYER_URL")
                .unwrap()
                .trim_end_matches('/')
                .to_string(),
            synchronizer_url: env::var("SYNCHRONIZER_URL")
                .unwrap()
                .trim_end_matches('/')
                .to_string(),
            client_ref: client_ref.to_string(),
        }
    }

    /// A witness that only pins the current state root, enough for actions without inputs.
    pub fn grounding_witness_without_inputs(&self) -> GroundingWitness {
        let response: GroundingWitnessResponse = self
            .http
            .post(format!(
                "{}/v1/txlib/grounding-witness",
                self.synchronizer_url
            ))
            .json(&GroundingWitnessRequest {
                object_commitments: Vec::new(),
            })
            .send()
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .unwrap();
        let state_header = StateHeader::new(
            response.block_number,
            response.block_timestamp,
            response.block_hash,
            response.created_root,
            response.nullifiers_root,
            response.prior_state_history_root,
        );
        assert_eq!(state_header.hash(), response.state_root);
        GroundingWitness::new(state_header, HashMap::new())
    }

    pub fn submit(&self, tx: PreparedTx) -> SubmittedTx {
        let response: SubmitProofResponse = self
            .http
            .post(format!("{}/api/v1/proofs", self.relayer_url))
            .json(&SubmitProofRequest {
                payload_base64: STANDARD.encode(&tx.payload),
                client_ref: Some(self.client_ref.clone()),
            })
            .send()
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .unwrap();
        assert_ne!(
            response.status,
            JobStatus::Failed,
            "relayer rejected job {}",
            response.job_id
        );
        SubmittedTx {
            job_id: response.job_id,
            created: tx.created,
            nullifiers: tx.nullifiers,
            landed_after: None,
        }
    }

    /// Poll the synchronizer until every created object and nullifier of every transaction is
    /// in its state, recording when each transaction became complete.
    pub fn wait_for_landing(&self, txs: &mut [SubmittedTx], timer: Instant) {
        let mut last_relayer_check = Instant::now();
        loop {
            let pending: Vec<usize> = (0..txs.len())
                .filter(|&index| txs[index].landed_after.is_none())
                .collect();
            if pending.is_empty() {
                return;
            }
            assert!(
                timer.elapsed() < LANDING_TIMEOUT,
                "{} of {} transactions did not land within {LANDING_TIMEOUT:?}",
                pending.len(),
                txs.len()
            );

            for chunk in pending_chunks(txs, &pending) {
                let membership = self.query_membership(txs, chunk);
                for &index in chunk {
                    let tx = &mut txs[index];
                    let landed = tx.created.iter().all(|commitment| {
                        membership
                            .created_results
                            .iter()
                            .any(|entry| entry.commitment == *commitment && entry.present)
                    }) && tx.nullifiers.iter().all(|nullifier| {
                        membership
                            .nullifier_results
                            .iter()
                            .any(|entry| entry.nullifier == *nullifier && entry.present)
                    });
                    if landed {
                        let landed_after = timer.elapsed();
                        tx.landed_after = Some(landed_after);
                        println!(
                            "relayer job {} landed after {landed_after:.2?} (slot {})",
                            tx.job_id, membership.last_processed_slot
                        );
                    }
                }
            }

            if last_relayer_check.elapsed() >= RELAYER_CHECK_INTERVAL {
                for tx in txs.iter().filter(|tx| tx.landed_after.is_none()) {
                    let status = self.job_status(&tx.job_id);
                    assert_ne!(
                        status.status,
                        JobStatus::Failed,
                        "relayer job {} failed: {:?}",
                        tx.job_id,
                        status.last_error
                    );
                }
                last_relayer_check = Instant::now();
            }

            std::thread::sleep(POLL_INTERVAL);
        }
    }

    fn query_membership(&self, txs: &[SubmittedTx], chunk: &[usize]) -> MembershipResponse {
        self.http
            .post(format!("{}/v1/state/membership", self.synchronizer_url))
            .json(&MembershipRequest {
                object_commitments: chunk
                    .iter()
                    .flat_map(|&index| txs[index].created.iter().copied())
                    .collect(),
                nullifiers: chunk
                    .iter()
                    .flat_map(|&index| txs[index].nullifiers.iter().copied())
                    .collect(),
            })
            .send()
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .unwrap()
    }

    fn job_status(&self, job_id: &str) -> JobStatusResponse {
        self.http
            .get(format!("{}/api/v1/proofs/{job_id}", self.relayer_url))
            .send()
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .unwrap()
    }
}

/// Split the pending transactions into groups whose hashes fit in one membership query.
fn pending_chunks<'a>(txs: &[SubmittedTx], pending: &'a [usize]) -> Vec<&'a [usize]> {
    let mut chunks = Vec::new();
    let mut chunk_start = 0;
    let mut chunk_hashes = 0;
    for (position, &index) in pending.iter().enumerate() {
        let tx_hashes = txs[index].created.len() + txs[index].nullifiers.len();
        if chunk_hashes + tx_hashes > MEMBERSHIP_QUERY_LIMIT {
            chunks.push(&pending[chunk_start..position]);
            chunk_start = position;
            chunk_hashes = 0;
        }
        chunk_hashes += tx_hashes;
    }
    chunks.push(&pending[chunk_start..]);
    chunks
}

#[cfg(test)]
mod tests {
    use pod2::middleware::EMPTY_HASH;

    use super::*;

    #[test]
    fn pending_chunks_respect_the_query_limit() {
        let tx = |hashes: usize| SubmittedTx {
            job_id: String::new(),
            created: vec![EMPTY_HASH; hashes],
            nullifiers: Vec::new(),
            landed_after: None,
        };
        let txs = vec![tx(200), tx(56), tx(1), tx(256)];
        let pending = [0, 1, 2, 3];
        assert_eq!(
            pending_chunks(&txs, &pending),
            vec![&[0, 1][..], &[2][..], &[3][..]]
        );
    }
}
