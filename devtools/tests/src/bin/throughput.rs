//! Proves `n` FindLog transactions up front, then measures how long the relayer and the
//! synchronizer take to land all of them.
//!
//! Usage: `throughput <n>`, with `RELAYER_URL` and `SYNCHRONIZER_URL` set in this crate's
//! `.env` (see `.env.example`).

use std::{
    collections::HashMap,
    env,
    path::PathBuf,
    time::{Duration, Instant},
};

use base64::{Engine, engine::general_purpose::STANDARD};
use driver::{DriverActionCatalog, PexeCatalog};
use payload::{
    payload::{Payload, PayloadProof},
    shrink::{ShrunkMainPodBuild, ShrunkMainPodSetup, shrink_compress_pod},
};
use pod2::middleware::{Hash, Params};
use reqwest::blocking::Client;
use sdk::SpendableObjects;
use txlib::{GroundingWitness, StateHeader};
use wire_types::{
    QualifiedName,
    relayer::{JobStatus, JobStatusResponse, SubmitProofRequest, SubmitProofResponse},
    synchronizer::{
        GroundingWitnessRequest, GroundingWitnessResponse, MembershipRequest, MembershipResponse,
    },
};

const PLUGIN_NAME: &str = "throughput";
const PLUGIN_MANIFEST: &str = include_str!("../../plugins/throughput/manifest.toml");
const PLUGIN_SCRIPT: &str = include_str!("../../plugins/throughput/plugin.rhai");

const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// How often unlanded transactions are checked for a failed relayer job, so a rejected proof
/// stops the run instead of waiting out `LANDING_TIMEOUT`.
const RELAYER_CHECK_INTERVAL: Duration = Duration::from_secs(15);
const LANDING_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// The synchronizer rejects membership queries with more hashes than this.
const MEMBERSHIP_QUERY_LIMIT: usize = 256;

struct PreparedTx {
    payload: Vec<u8>,
    created: Vec<Hash>,
    nullifiers: Vec<Hash>,
}

struct SubmittedTx {
    job_id: String,
    created: Vec<Hash>,
    nullifiers: Vec<Hash>,
    landed_after: Option<Duration>,
}

fn main() {
    env_logger::init();
    dotenvy::from_path(concat!(env!("CARGO_MANIFEST_DIR"), "/.env")).unwrap();
    let relayer_url = env::var("RELAYER_URL").unwrap();
    let synchronizer_url = env::var("SYNCHRONIZER_URL").unwrap();
    let num_txs: usize = env::args()
        .nth(1)
        .expect("usage: throughput <n>")
        .parse()
        .unwrap();

    let http = Client::new();
    let catalog = load_catalog(false);
    let action = QualifiedName::new(PLUGIN_NAME, "FindLog");
    println!("building the shrink circuit...");
    let shrink_build = ShrunkMainPodSetup::new(&Params::default()).build().unwrap();

    let prepared: Vec<PreparedTx> = (0..num_txs)
        .map(|index| {
            let started = Instant::now();
            let witness = fetch_grounding_witness(&http, &synchronizer_url);
            let state_root = witness.state_header.hash();
            let outputs = catalog
                .execute_action(action.clone(), witness, Vec::new())
                .unwrap();
            let prepared = prepare_tx(&shrink_build, state_root, &outputs);
            println!(
                "proved tx {}/{num_txs} in {:.1?}",
                index + 1,
                started.elapsed()
            );
            prepared
        })
        .collect();

    let timer = Instant::now();
    let mut submitted: Vec<SubmittedTx> = prepared
        .into_iter()
        .enumerate()
        .map(|(index, tx)| {
            let job_id = submit(&http, &relayer_url, &tx.payload);
            println!(
                "submitted tx {}/{num_txs} as relayer job {job_id}",
                index + 1
            );
            SubmittedTx {
                job_id,
                created: tx.created,
                nullifiers: tx.nullifiers,
                landed_after: None,
            }
        })
        .collect();
    let submission_time = timer.elapsed();

    wait_for_landing(
        &http,
        &relayer_url,
        &synchronizer_url,
        &mut submitted,
        timer,
    );
    let total_time = timer.elapsed();

    println!();
    println!("transactions:     {num_txs}");
    println!("submission time:  {submission_time:.2?}");
    println!("time to land all: {total_time:.2?}");
    println!(
        "throughput:       {:.3} tx/s",
        num_txs as f64 / total_time.as_secs_f64()
    );
    let mut landing_times: Vec<Duration> = submitted
        .iter()
        .map(|tx| tx.landed_after.unwrap())
        .collect();
    landing_times.sort();
    println!("first landed at:  {:.2?}", landing_times[0]);
    println!(
        "median landed at: {:.2?}",
        landing_times[landing_times.len() / 2]
    );
}

/// Pack the plugin with the module hash its script compiles to.
fn load_catalog(mock_proofs: bool) -> PexeCatalog {
    let manifest: sdk::manifest::Manifest = toml::from_str(PLUGIN_MANIFEST).unwrap();
    let module_hash = pexe::compile_module_hash(&manifest, PLUGIN_SCRIPT).unwrap();
    let manifest_toml = pexe::set_manifest_hash(PLUGIN_MANIFEST, &module_hash).unwrap();
    let bytes = pexe::pack(&manifest_toml, PLUGIN_SCRIPT).unwrap();
    PexeCatalog::from_bytes(
        [(PathBuf::from(format!("{PLUGIN_NAME}.pexe")), bytes)],
        mock_proofs,
    )
    .unwrap()
}

/// FindLog spends no inputs, so the witness only pins the state root it is grounded on.
fn fetch_grounding_witness(http: &Client, synchronizer_url: &str) -> GroundingWitness {
    let response: GroundingWitnessResponse = http
        .post(format!(
            "{}/v1/txlib/grounding-witness",
            synchronizer_url.trim_end_matches('/')
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

fn prepare_tx(
    shrink_build: &ShrunkMainPodBuild,
    state_root: Hash,
    outputs: &SpendableObjects,
) -> PreparedTx {
    let compressed = shrink_compress_pod(shrink_build, outputs.tx_pod.clone()).unwrap();
    let nullifiers = outputs.tx.nullifier_hashes().unwrap();
    let payload = Payload {
        proof: PayloadProof::Plonky2(Box::new(compressed)),
        tx_final: outputs.tx.dict().commitment(),
        state_root,
        nullifiers: nullifiers.clone(),
        live: outputs.tx.live_commitments().unwrap(),
    };
    PreparedTx {
        payload: payload.to_bytes(),
        created: outputs
            .objs
            .iter()
            .map(|spendable| spendable.obj.commitment())
            .collect(),
        nullifiers,
    }
}

fn submit(http: &Client, relayer_url: &str, payload: &[u8]) -> String {
    let response: SubmitProofResponse = http
        .post(format!(
            "{}/api/v1/proofs",
            relayer_url.trim_end_matches('/')
        ))
        .json(&SubmitProofRequest {
            payload_base64: STANDARD.encode(payload),
            client_ref: Some("throughput".to_string()),
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
    response.job_id
}

/// Poll the synchronizer until every created object and nullifier of every transaction is in
/// its state, recording when each transaction became complete.
fn wait_for_landing(
    http: &Client,
    relayer_url: &str,
    synchronizer_url: &str,
    txs: &mut [SubmittedTx],
    timer: Instant,
) {
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
            let membership = query_membership(http, synchronizer_url, txs, chunk);
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
                let status = job_status(http, relayer_url, &tx.job_id);
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

fn query_membership(
    http: &Client,
    synchronizer_url: &str,
    txs: &[SubmittedTx],
    chunk: &[usize],
) -> MembershipResponse {
    http.post(format!(
        "{}/v1/state/membership",
        synchronizer_url.trim_end_matches('/')
    ))
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

fn job_status(http: &Client, relayer_url: &str, job_id: &str) -> JobStatusResponse {
    http.get(format!(
        "{}/api/v1/proofs/{job_id}",
        relayer_url.trim_end_matches('/')
    ))
    .send()
    .unwrap()
    .error_for_status()
    .unwrap()
    .json()
    .unwrap()
}

#[cfg(test)]
mod tests {
    use pod2::middleware::EMPTY_HASH;

    use super::*;

    fn dummy_grounding_witness() -> GroundingWitness {
        GroundingWitness::new(
            StateHeader::new(1, 1, EMPTY_HASH, EMPTY_HASH, EMPTY_HASH, EMPTY_HASH),
            HashMap::new(),
        )
    }

    /// Every FindLog must create a distinct object, or later transactions would collide with
    /// earlier ones in the created set.
    #[test]
    fn find_log_creates_a_distinct_object_each_run() {
        let catalog = load_catalog(true);
        let action = QualifiedName::new(PLUGIN_NAME, "FindLog");
        let run = || {
            let outputs = catalog
                .execute_action(action.clone(), dummy_grounding_witness(), Vec::new())
                .unwrap();
            assert_eq!(outputs.objs.len(), 1);
            assert!(outputs.tx.nullifier_hashes().unwrap().is_empty());
            outputs.objs[0].obj.commitment()
        };
        assert_ne!(run(), run());
    }

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
