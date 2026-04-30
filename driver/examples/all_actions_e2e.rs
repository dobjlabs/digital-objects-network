//! Chain all 5 craft-basics actions through `Driver::execute_named` end to
//! end. Uses a more realistic mock synchronizer that maintains a real
//! SHA-256 SMT — so the second action grounds against the first action's
//! tx_final, the third against the second, and so on.
//!
//! Usage:
//! ```bash
//! RISC0_DEV_MODE=1 cargo run -p driver --release --example all_actions_e2e
//! ```
//!
//! Sequence:
//! ```
//!  1. FindLog                          → Log_a
//!  2. CraftWood(Log_a)                 → Wood_a
//!  3. CraftSticks(Wood_a)              → Stick_1, Stick_2
//!  4. FindLog                          → Log_b           // need another Wood for the pick
//!  5. CraftWood(Log_b)                 → Wood_b
//!  6. CraftWoodPick(Wood_b, Stick_1)   → Pick
//!  7. UseWoodPick(Pick)                → Pick'           // mutation, durability 99
//! ```
//!
//! 7 proofs; ~150ms each in dev mode → ~1s total.

use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use driver::{
    Driver, ObjectStatus,
    clients::{
        GroundingWitness, JobStatus, RelayJobStatus, RelaySubmission, RelayerClient,
        SourceTxWitness, SyncStateHead, SynchronizerClient,
    },
    driver::{DriverDeps, ObjectSelector},
    execute::Risc0Prover,
    object::ObjectRecord,
    paths::DriverPaths,
    settings::default_settings,
};
use tempfile::tempdir;
use txlib_core::Hash;
use txlib_core::merkle_store::{InMemoryNodeStore, PersistentSmt, empty_root};
use txlib_core::tx::StateRoot;

// ===========================================================================
// Realistic mock synchronizer: maintains a real SHA-256 SMT
// ===========================================================================

struct MockSynchronizer {
    store: InMemoryNodeStore,
    transactions_root: Mutex<Hash>,
    block_number: Mutex<i64>,
}

impl MockSynchronizer {
    fn new() -> Self {
        Self {
            store: InMemoryNodeStore::new(),
            transactions_root: Mutex::new(empty_root()),
            block_number: Mutex::new(1),
        }
    }

    fn confirm(&self, tx_final: Hash) -> Result<()> {
        let mut root = self.transactions_root.lock().unwrap();
        let mut smt = PersistentSmt::open(*root, &self.store);
        smt.insert(tx_final, tx_final)
            .map_err(|e| anyhow::anyhow!("smt insert: {e}"))?;
        *root = smt.root;
        *self.block_number.lock().unwrap() += 1;
        Ok(())
    }

    fn current_state_root(&self) -> StateRoot {
        StateRoot::new(
            *self.block_number.lock().unwrap(),
            *self.transactions_root.lock().unwrap(),
            empty_root(),
            Hash::default(),
        )
    }
}

impl SynchronizerClient for MockSynchronizer {
    fn state_head(&self) -> Result<SyncStateHead> {
        let sr = self.current_state_root();
        Ok(SyncStateHead {
            last_processed_slot: *self.block_number.lock().unwrap() as u32,
            current_gsr: Some(sr.hash()),
            tx_count: 0,
            nullifier_count: 0,
        })
    }

    fn grounding_witness(&self, source_tx_hashes: &[Hash]) -> Result<GroundingWitness> {
        let sr = self.current_state_root();
        let smt = PersistentSmt::open(sr.transactions_root, &self.store);
        let witnesses = source_tx_hashes
            .iter()
            .map(|h| {
                let proof = smt.prove(*h).map_err(|e| anyhow::anyhow!("{e}"))?;
                let present = smt.contains_set_member(*h).map_err(|e| anyhow::anyhow!("{e}"))?;
                Ok(SourceTxWitness {
                    source_tx_final: *h,
                    present,
                    tx_inclusion_proof: proof,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(GroundingWitness {
            state_root_hash: sr.hash(),
            block_number: sr.block_number,
            transactions_root: sr.transactions_root,
            nullifiers_root: sr.nullifiers_root,
            gsrs_root: sr.gsrs_root,
            witnesses,
        })
    }

    fn tx_present(&self, tx_final: Hash) -> Result<bool> {
        let smt = PersistentSmt::open(*self.transactions_root.lock().unwrap(), &self.store);
        smt.contains_set_member(tx_final)
            .map_err(|e| anyhow::anyhow!("{e}"))
    }
}

// ===========================================================================
// Mock relayer: extracts tx_final from the receipt, tells the synchronizer
// ===========================================================================

struct MockRelayer {
    sync: Arc<MockSynchronizer>,
    next_id: Mutex<u64>,
    last_submitted: Mutex<Option<RelaySubmission>>,
}

impl MockRelayer {
    fn new(sync: Arc<MockSynchronizer>) -> Self {
        Self {
            sync,
            next_id: Mutex::new(1),
            last_submitted: Mutex::new(None),
        }
    }
}

impl RelayerClient for MockRelayer {
    fn submit_payload(
        &self,
        payload_bytes: &[u8],
        _client_ref: Option<&str>,
    ) -> Result<RelaySubmission> {
        let receipt_bytes = common::payload::decode_blob_envelope(payload_bytes)?
            .context("payload missing magic envelope")?;
        let receipt: risc0_zkvm::Receipt = bincode::deserialize(receipt_bytes)?;
        let journal: txlib_core::abi::GuestJournal = borsh::from_slice(&receipt.journal.bytes)?;

        self.sync.confirm(journal.tx_final)?;

        let mut id = self.next_id.lock().unwrap();
        let submission = RelaySubmission {
            job_id: format!("job-{}", *id),
            status: JobStatus::Submitted,
            tx_final: journal.tx_final,
            state_root_hash: journal.state_root_hash,
        };
        *id += 1;
        *self.last_submitted.lock().unwrap() = Some(submission.clone());
        Ok(submission)
    }

    fn job_status(&self, job_id: &str) -> Result<RelayJobStatus> {
        let submitted = self
            .last_submitted
            .lock()
            .unwrap()
            .clone()
            .context("no job submitted")?;
        // For the e2e we treat every submission as instantly Confirmed by the
        // time the driver polls. Real relayer would take seconds; the driver's
        // poll loop handles either.
        Ok(RelayJobStatus {
            job_id: job_id.to_string(),
            status: JobStatus::Confirmed,
            tx_hash: Some(format!("0x{:0>64}", &job_id["job-".len()..])),
            block_number: Some(100 + job_id["job-".len()..].parse::<u64>().unwrap_or(0)),
            last_error: None,
            tx_final: submitted.tx_final,
        })
    }
}

// ===========================================================================
// E2E
// ===========================================================================

fn find_one(driver: &Driver, class: &str) -> Result<ObjectRecord> {
    driver
        .list_objects()?
        .into_iter()
        .find(|r| r.class_name == class && r.status == ObjectStatus::Live)
        .ok_or_else(|| anyhow::anyhow!("no Live {class} on disk"))
}

fn find_all(driver: &Driver, class: &str) -> Result<Vec<ObjectRecord>> {
    Ok(driver
        .list_objects()?
        .into_iter()
        .filter(|r| r.class_name == class && r.status == ObjectStatus::Live)
        .collect())
}

fn run(name: &str, driver: &Driver, selectors: Vec<ObjectSelector>) -> Result<()> {
    let start = std::time::Instant::now();
    let result = driver.execute_named(name, selectors)?;
    eprintln!(
        "  {name:<16} → tx_final {}…  ({:.0}ms; outputs={}, nullified={})",
        &result.tx_final[..18],
        start.elapsed().as_millis(),
        result.output_files.len(),
        result.nullified_files.len(),
    );
    Ok(())
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::filter::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn".into()),
        )
        .init();

    let dir = tempdir()?;
    let paths = DriverPaths::from_dobj_root(dir.path());

    let sync = Arc::new(MockSynchronizer::new());
    let relayer = Arc::new(MockRelayer::new(Arc::clone(&sync)));
    let prover = Arc::new(Risc0Prover);

    let driver = Driver::open(
        paths,
        default_settings(),
        DriverDeps {
            synchronizer: sync.clone(),
            relayer,
            prover,
        },
    );

    eprintln!("== chaining all 5 craft-basics actions ==");
    let all_start = std::time::Instant::now();

    // 1. FindLog → Log_a
    run("FindLog", &driver, vec![])?;
    let log_a = find_one(&driver, "Log")?;

    // 2. CraftWood(Log_a) → Wood_a
    run(
        "CraftWood",
        &driver,
        vec![ObjectSelector::Id(log_a.id.clone())],
    )?;
    let wood_a = find_one(&driver, "Wood")?;

    // 3. CraftSticks(Wood_a) → Stick_1, Stick_2
    run(
        "CraftSticks",
        &driver,
        vec![ObjectSelector::Id(wood_a.id.clone())],
    )?;
    let sticks = find_all(&driver, "Stick")?;
    assert_eq!(sticks.len(), 2, "CraftSticks should produce two");

    // 4 + 5. Need another Wood for the pick — repeat FindLog + CraftWood.
    run("FindLog", &driver, vec![])?;
    let log_b = find_one(&driver, "Log")?;
    run(
        "CraftWood",
        &driver,
        vec![ObjectSelector::Id(log_b.id.clone())],
    )?;
    let wood_b = find_one(&driver, "Wood")?;

    // 6. CraftWoodPick(Wood_b, Stick_1) → Pick
    run(
        "CraftWoodPick",
        &driver,
        vec![
            ObjectSelector::Id(wood_b.id.clone()),
            ObjectSelector::Id(sticks[0].id.clone()),
        ],
    )?;
    let pick = find_one(&driver, "WoodPick")?;
    let pick_durability_initial = match pick.obj.fields.get("durability") {
        Some(txlib_core::value::Value::Int(n)) => *n,
        _ => panic!("WoodPick missing durability"),
    };
    assert_eq!(pick_durability_initial, 100);

    // 7. UseWoodPick(Pick) → Pick'
    run(
        "UseWoodPick",
        &driver,
        vec![ObjectSelector::Id(pick.id.clone())],
    )?;
    let pick_after = find_one(&driver, "WoodPick")?;
    let pick_durability_after = match pick_after.obj.fields.get("durability") {
        Some(txlib_core::value::Value::Int(n)) => *n,
        _ => panic!("WoodPick missing durability"),
    };
    assert_eq!(pick_durability_after, 99);
    assert_ne!(pick.id, pick_after.id, "mutation must change the id");

    eprintln!(
        "== done: 7 proofs in {:.1}s ==",
        all_start.elapsed().as_secs_f32()
    );
    eprintln!();
    eprintln!("== final live inventory ==");
    let mut live = driver.list_objects()?;
    live.sort_by_key(|r| r.class_name.clone());
    for r in live {
        eprintln!(
            "  {:<10}  {} status={:?}",
            r.class_name,
            &r.id[..18],
            r.status
        );
    }

    Ok(())
}
