//! End-to-end driver smoke test: drive a real risc0 prover (dev mode)
//! through `Driver::execute(FindLog, ...)` with in-process mock HTTP
//! clients, and verify the output `.dobj` ends up on disk.
//!
//! Usage:
//! ```bash
//! RISC0_DEV_MODE=1 cargo run -p driver --example find_log_e2e
//! ```
//!
//! Real proving (drop the env var) takes minutes; dev mode takes ~150ms.
//!
//! What this exercises:
//! - Driver::build_plan grabs the (mocked) state head
//! - Driver runs `craft_actions::validate` on the host as a sanity check
//! - Risc0Prover invokes the actual guest ELF, decodes the journal
//! - Driver writes the output object as `.dobj` (status: Unknown → Pending → Live)
//! - Mock relayer reports Confirmed
//! - Mock synchronizer reports tx_present=true
//! - Output file ends in status `Live` with the eth tx hash populated

use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use driver::{
    Driver, ObjectStatus, all_actions,
    catalog::action_by_name,
    clients::{
        GroundingWitness, HttpRelayerClient, JobStatus, RelayJobStatus, RelaySubmission,
        RelayerClient, SyncStateHead, SynchronizerClient,
    },
    driver::{ActionStaging, ExecuteActionInput, ObjectSelector},
    execute::Risc0Prover,
    paths::DriverPaths,
    settings::default_settings,
};
use tempfile::tempdir;
use txlib_core::Hash;
use txlib_core::abi::IntroWitness;
use txlib_core::hash::sha256;
use txlib_core::merkle::set_smt_root;
use txlib_core::merkle_store::empty_root;
use txlib_core::object;
use txlib_core::tx::StateRoot;

const FIND_LOG_VDF_ITERS: u32 = 3;

// ===========================================================================
// Mocks
// ===========================================================================

struct MockSynchronizer {
    confirmed_tx_finals: Mutex<std::collections::HashSet<Hash>>,
    state_root: StateRoot,
    state_root_hash: Hash,
}

impl MockSynchronizer {
    fn new() -> Self {
        let state_root = StateRoot::new(1, empty_root(), empty_root(), Hash::default());
        let state_root_hash = state_root.hash();
        Self {
            confirmed_tx_finals: Mutex::new(Default::default()),
            state_root,
            state_root_hash,
        }
    }

    fn confirm(&self, tx_final: Hash) {
        self.confirmed_tx_finals.lock().unwrap().insert(tx_final);
    }
}

impl SynchronizerClient for MockSynchronizer {
    fn state_head(&self) -> Result<SyncStateHead> {
        Ok(SyncStateHead {
            last_processed_slot: 100,
            current_gsr: Some(self.state_root_hash),
            tx_count: 0,
            nullifier_count: 0,
        })
    }

    fn grounding_witness(&self, source_tx_hashes: &[Hash]) -> Result<GroundingWitness> {
        // FindLog has no inputs, so this gets called with an empty slice and
        // is only used to learn the canonical roots.
        let _ = source_tx_hashes;
        Ok(GroundingWitness {
            state_root_hash: self.state_root_hash,
            block_number: self.state_root.block_number,
            transactions_root: self.state_root.transactions_root,
            nullifiers_root: self.state_root.nullifiers_root,
            gsrs_root: self.state_root.gsrs_root,
            witnesses: Vec::new(),
        })
    }

    fn tx_present(&self, tx_final: Hash) -> Result<bool> {
        Ok(self.confirmed_tx_finals.lock().unwrap().contains(&tx_final))
    }
}

struct MockRelayer {
    sync: Arc<MockSynchronizer>,
    submitted: Mutex<Option<RelaySubmission>>,
}

impl MockRelayer {
    fn new(sync: Arc<MockSynchronizer>) -> Self {
        Self {
            sync,
            submitted: Mutex::new(None),
        }
    }
}

impl RelayerClient for MockRelayer {
    fn submit_payload(
        &self,
        payload_bytes: &[u8],
        _client_ref: Option<&str>,
    ) -> Result<RelaySubmission> {
        // Decode the magic envelope + receipt to extract tx_final from the
        // journal (mirroring what a real relayer would do).
        let receipt_bytes = common::payload::decode_blob_envelope(payload_bytes)?
            .context("payload missing magic envelope")?;
        let receipt: risc0_zkvm::Receipt = bincode::deserialize(receipt_bytes)?;
        let journal: txlib_core::abi::GuestJournal =
            borsh::from_slice(&receipt.journal.bytes)?;

        // Pretend the blob was confirmed instantly + tell the synchronizer.
        self.sync.confirm(journal.tx_final);

        let submission = RelaySubmission {
            job_id: "mock-job-1".to_string(),
            status: JobStatus::Submitted,
            tx_final: journal.tx_final,
            state_root_hash: journal.state_root_hash,
        };
        *self.submitted.lock().unwrap() = Some(submission.clone());
        Ok(submission)
    }

    fn job_status(&self, job_id: &str) -> Result<RelayJobStatus> {
        let submitted = self
            .submitted
            .lock()
            .unwrap()
            .clone()
            .context("no job submitted")?;
        Ok(RelayJobStatus {
            job_id: job_id.to_string(),
            status: JobStatus::Confirmed,
            tx_hash: Some("0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef".into()),
            block_number: Some(12345),
            last_error: None,
            tx_final: submitted.tx_final,
        })
    }
}

// ===========================================================================
// Build the FindLog action's staging
// ===========================================================================

fn run_vdf(iters: u32, input: Hash) -> Hash {
    let mut current = input;
    for _ in 0..iters {
        current = sha256(current.as_bytes());
    }
    current
}

fn build_find_log_staging() -> ActionStaging {
    let key = sha256(b"e2e-test-key");
    let mut log = object! {
        "blueprint" => "Log",
        "key" => key,
    };
    let vdf_input = log.commitment();
    let work = run_vdf(FIND_LOG_VDF_ITERS, vdf_input);
    log.insert("work", work);

    ActionStaging {
        new_objects: vec![log],
        intro_witnesses: vec![IntroWitness::Vdf {
            iters: FIND_LOG_VDF_ITERS,
            input: vdf_input,
            output: work,
        }],
        new_object_classes: vec!["Log".to_string()],
    }
}

// ===========================================================================
// Main
// ===========================================================================

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::filter::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    eprintln!("--- Available actions ---");
    for a in all_actions() {
        eprintln!(
            "  {:>2}  {} {:18}  {} → {}",
            a.id,
            a.emoji,
            a.name,
            a.inputs.join("+").as_str(),
            a.outputs.join("+").as_str()
        );
    }

    let dir = tempdir()?;
    let paths = DriverPaths::from_dobj_root(dir.path());
    eprintln!("--- DriverPaths under {} ---", dir.path().display());

    let sync = Arc::new(MockSynchronizer::new());
    let relayer = Arc::new(MockRelayer::new(Arc::clone(&sync)));
    let prover = Arc::new(Risc0Prover);

    let driver = Driver::open(
        paths,
        default_settings(),
        driver::driver::DriverDeps {
            synchronizer: sync.clone(),
            relayer,
            prover,
        },
    );

    let action = action_by_name("FindLog").context("FindLog catalog entry missing")?;
    let staging = build_find_log_staging();

    eprintln!("--- Executing FindLog ---");
    let start = std::time::Instant::now();
    let result = driver.execute(ExecuteActionInput {
        action_id: action.id,
        input_selectors: Vec::<ObjectSelector>::new(),
        staging,
    })?;
    eprintln!("execute: {:?}", start.elapsed());

    eprintln!(
        "tx_final         = {}\nrelayer_job_id   = {}\ntx_hash          = {:?}\nblock_number     = {:?}\noutput_files     = {:?}\nnullified_files  = {:?}",
        result.tx_final,
        result.relayer_job_id,
        result.tx_hash,
        result.block_number,
        result.output_files,
        result.nullified_files,
    );

    eprintln!("--- Final on-disk state ---");
    let records = driver.list_objects()?;
    assert_eq!(records.len(), 1, "expected one new Log on disk");
    let log = &records[0];
    eprintln!(
        "  {} status={:?} tx={:?} fields={:?}",
        log.id,
        log.status,
        log.tx_hash,
        log.obj.fields.keys().collect::<Vec<_>>()
    );
    assert_eq!(log.class_name, "Log");
    assert_eq!(log.status, ObjectStatus::Live);
    assert!(log.tx_hash.is_some());

    // Sanity: the live_inclusion_proof we'd build from this record verifies
    // against the source_tx.live_root we wrote.
    let proof = log.live_inclusion_proof()?;
    let expected_live_root = set_smt_root(&log.source_tx_live);
    assert_eq!(expected_live_root, log.source_tx.live_root);
    assert!(txlib_core::merkle::verify_inclusion(
        log.source_tx.live_root,
        log.commitment(),
        log.commitment(),
        &proof,
    ));
    eprintln!("✓ live inclusion proof verifies");

    Ok(())
}

// Linker bait — keeps the unused HTTP client out of the example binary.
#[allow(dead_code)]
fn _unused() {
    let _ = HttpRelayerClient::new("");
}
