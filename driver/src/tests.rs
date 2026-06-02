use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Result, anyhow};
use common::test_state::TestState;
use pod2::middleware::Hash;
use sdk::SpendableObjects;
use tempfile::tempdir;
use txlib::{GroundingWitness, StateRoot};

use crate::catalog::ActionCatalog;
use crate::clients::{
    RelayerClient, RelayerConfirmation, SynchronizerClient, SynchronizerHead,
    SynchronizerMembership,
};
use crate::driver::{Driver, DriverDeps, PayloadBuilder};
use crate::object_record::{ObjectRecord, ensure_extra_pod_deserializers_registered};
use crate::object_store::{
    ObjectFileEntry, ensure_store_dirs, load_object_files, write_object_file,
};
use crate::pexe_catalog::{PexeCatalog, test_plugin_bytes};
use crate::{ActionQuery, DriverPaths, ExecuteActionInput};
use wire_types::{ObjectStatus, QualifiedName};

fn temp_paths() -> DriverPaths {
    let dir = tempdir().unwrap();
    DriverPaths::from_dobj_root(dir.keep())
}

fn make_catalog() -> PexeCatalog {
    PexeCatalog::from_bytes(
        std::iter::once((
            std::path::PathBuf::from("craft-basics.pexe"),
            test_plugin_bytes(),
        )),
        true,
    )
    .expect("catalog loads from test plugin bytes")
}

fn dummy_grounding_witness() -> GroundingWitness {
    GroundingWitness::new(
        StateRoot::new(
            1,
            pod2::middleware::EMPTY_HASH,
            pod2::middleware::EMPTY_HASH,
            pod2::middleware::EMPTY_HASH,
        ),
        HashMap::new(),
    )
}

fn tx_hash(tx: &txlib::Tx) -> Hash {
    tx.dict().commitment()
}

fn tx_nullifiers(tx: &txlib::Tx) -> Vec<Hash> {
    tx.nullifiers
        .iter()
        .map(|nullifier| {
            let nullifier = nullifier.expect("tx nullifier should decode");
            Hash(nullifier.raw().0)
        })
        .collect()
}

fn apply_tx(state: &mut TestState, tx: &txlib::Tx) {
    state.apply_tx(tx_hash(tx), tx_nullifiers(tx));
}

fn state_root(state: &TestState) -> StateRoot {
    let (transactions_root, nullifiers_root, gsrs_root) = state.roots();
    StateRoot::new(
        state.block_number,
        transactions_root,
        nullifiers_root,
        gsrs_root,
    )
}

fn craft_basics(name: &str) -> QualifiedName {
    QualifiedName::new("craft-basics", name)
}

fn make_input_record(file_name: &str) -> (ObjectFileEntry, DriverDeps) {
    ensure_extra_pod_deserializers_registered();
    let catalog = make_catalog();
    let outputs = catalog
        .execute_action(craft_basics("FindLog"), dummy_grounding_witness(), vec![])
        .unwrap();
    let source_tx = outputs.tx.clone();
    let spendable = outputs.obj(0);
    let id = format!("{:#}", spendable.obj.commitment());
    let record = ObjectRecord {
        content_hash: id,
        class: craft_basics("Log"),
        status: ObjectStatus::Live,
        tx_hash: None,
        obj: spendable.obj,
        evidence: spendable.evidence,
    };
    let mut state = TestState::default();
    apply_tx(&mut state, &source_tx);
    (
        ObjectFileEntry {
            file_name: file_name.to_string(),
            record,
        },
        DriverDeps {
            catalog: Arc::new(catalog),
            synchronizer: Arc::new(MockSynchronizer {
                state,
                ..MockSynchronizer::default()
            }),
            relayer: Arc::new(MockRelayer::default()),
            payload_builder: Arc::new(MockPayloadBuilder),
        },
    )
}

struct MockPayloadBuilder;

impl PayloadBuilder for MockPayloadBuilder {
    fn build_payload(
        &self,
        _old_state_root_hash: &Hash,
        _action_output: &SpendableObjects,
    ) -> Result<Vec<u8>> {
        Ok(vec![1, 2, 3])
    }
}

#[derive(Default)]
struct MockSynchronizer {
    fail_wait: bool,
    state: TestState,
    membership_grounded: HashSet<Hash>,
    membership_nullifiers: HashSet<Hash>,
    fail_membership: bool,
}

impl SynchronizerClient for MockSynchronizer {
    fn fetch_head(&self, _sync_api_url: &str) -> Result<SynchronizerHead> {
        Ok(SynchronizerHead {
            current_gsr: state_root(&self.state).hash(),
        })
    }

    fn fetch_grounding_witness(
        &self,
        _sync_api_url: &str,
        source_tx_hashes: &[Hash],
    ) -> Result<GroundingWitness> {
        let source_tx_proofs = source_tx_hashes
            .iter()
            .copied()
            .map(|tx_hash| (tx_hash, self.state.tx_membership_proof(tx_hash)))
            .collect::<HashMap<_, _>>();
        Ok(GroundingWitness::new(
            state_root(&self.state),
            source_tx_proofs,
        ))
    }

    fn fetch_membership_with_nullifiers(
        &self,
        _sync_api_url: &str,
        _tx_hashes: &[Hash],
        _nullifiers: &[Hash],
    ) -> Result<SynchronizerMembership> {
        if self.fail_membership {
            return Err(anyhow!("synchronizer unreachable"));
        }
        Ok(SynchronizerMembership {
            grounded_txs: self.membership_grounded.clone(),
            on_chain_nullifiers: self.membership_nullifiers.clone(),
        })
    }

    fn wait_for_tx(
        &self,
        _sync_api_url: &str,
        _tx_final: Hash,
        _timeout_secs: u64,
        _poll_interval_ms: u64,
    ) -> Result<SynchronizerHead> {
        if self.fail_wait {
            return Err(anyhow!("synchronizer timeout"));
        }
        Ok(SynchronizerHead {
            current_gsr: state_root(&self.state).hash(),
        })
    }
}

#[derive(Default)]
struct MockRelayer {
    fail_submit: bool,
    fail_wait: bool,
}

impl RelayerClient for MockRelayer {
    fn submit_proof(
        &self,
        _relayer_api_url: &str,
        _payload_bytes: &[u8],
        _client_ref: Option<String>,
    ) -> Result<wire_types::relayer::SubmitProofResponse> {
        if self.fail_submit {
            return Err(anyhow!("relayer submit failed"));
        }
        Ok(wire_types::relayer::SubmitProofResponse {
            job_id: "job-1".to_string(),
            status: wire_types::relayer::JobStatus::Queued,
            tx_final: "0x0".to_string(),
            state_root_hash: "0x0".to_string(),
            attempt_count: 0,
            created_at: 0,
        })
    }

    fn wait_for_tx_hash(
        &self,
        _relayer_api_url: &str,
        _job_id: &str,
        _timeout_secs: u64,
        _poll_interval_ms: u64,
    ) -> Result<String> {
        if self.fail_wait {
            return Err(anyhow!("relayer timeout"));
        }
        Ok("0xtx".to_string())
    }

    fn wait_for_confirmation(
        &self,
        _relayer_api_url: &str,
        _job_id: &str,
        _timeout_secs: u64,
        _poll_interval_ms: u64,
    ) -> Result<RelayerConfirmation> {
        if self.fail_wait {
            return Err(anyhow!("relayer timeout"));
        }
        Ok(RelayerConfirmation {
            job_id: "job-1".to_string(),
            tx_hash: Some("0xtx".to_string()),
            block_number: Some(7),
        })
    }

    fn lookup_tx_hash(&self, _relayer_api_url: &str, _tx_final: &str) -> Result<Option<String>> {
        Ok(Some("0xtx".to_string()))
    }
}

#[test]
fn test_list_actions_filters_by_input_class() {
    // Use the in-memory test catalog so the assertion does not depend on
    // whatever pexe plugins happen to live under ~/.dobj/actions/.
    let paths = temp_paths();
    ensure_store_dirs(&paths).unwrap();
    let deps = DriverDeps {
        catalog: Arc::new(make_catalog()),
        synchronizer: Arc::new(MockSynchronizer::default()),
        relayer: Arc::new(MockRelayer::default()),
        payload_builder: Arc::new(MockPayloadBuilder),
    };
    let driver = Driver::open(paths, deps).unwrap();
    let wood = craft_basics("Wood");
    let filtered = driver
        .list_actions(Some(&ActionQuery {
            input_class: Some(wood.clone()),
            ..ActionQuery::default()
        }))
        .unwrap();
    assert!(
        !filtered.is_empty(),
        "expected at least one Wood-consuming action"
    );
    assert!(
        filtered
            .iter()
            .all(|action| action.total_inputs.iter().any(|r| r.class == wood))
    );
}

#[test]
fn test_execute_rolls_back_on_relayer_submit_failure() {
    let (entry, mut deps) = make_input_record("log_1.dobj");
    deps.relayer = Arc::new(MockRelayer {
        fail_submit: true,
        ..MockRelayer::default()
    });
    let paths = temp_paths();
    ensure_store_dirs(&paths).unwrap();
    write_object_file(&paths, &entry.record, &entry.file_name).unwrap();
    let driver = Driver::open(paths.clone(), deps).unwrap();

    let err = driver
        .execute(ExecuteActionInput {
            action: craft_basics("CraftWood"),
            input_objects: vec!["log_1.dobj".to_string()],
        })
        .unwrap_err();
    assert!(err.to_string().contains("relayer submit failed"));

    // Submission never reached the relayer. Output files are kept as Unknown
    // so the user can retry submission without regenerating proofs.
    let remaining = load_object_files(&paths).unwrap();
    assert_eq!(remaining.len(), 2);
    let input = remaining
        .iter()
        .find(|e| e.file_name == "log_1.dobj")
        .unwrap();
    assert!(!input.record.is_nullified());
    let output = remaining
        .iter()
        .find(|e| e.file_name != "log_1.dobj")
        .unwrap();
    assert_eq!(output.record.status, ObjectStatus::Unknown);
}

/// A `.dobj` whose `class` text matches what an action expects must STILL
/// be rejected when its on-chain `obj["type"]` predicate hash disagrees. This
/// is the regression test for the original collision bug: if two plugins
/// declared a class `Wood`, the bare-name check used to let the wrong
/// `IsWood@A` object satisfy `IsWood@B`'s requirement and proof generation
/// would burn minutes before the prover failed.
#[test]
fn test_execute_rejects_class_hash_mismatch_with_matching_class_id() {
    // A real Log object (obj["type"] = IsLog hash) written to disk with a
    // forged class of "craft-basics::Wood". CraftSticks consumes a Wood, so
    // the qualified-name check passes; the cryptographic hash check then fires.
    let (mut entry, deps) = make_input_record("forged_wood.dobj");
    entry.record.class = craft_basics("Wood");
    let paths = temp_paths();
    ensure_store_dirs(&paths).unwrap();
    write_object_file(&paths, &entry.record, &entry.file_name).unwrap();
    let driver = Driver::open(paths.clone(), deps).unwrap();

    let err = driver
        .execute(ExecuteActionInput {
            action: craft_basics("CraftSticks"),
            input_objects: vec!["forged_wood.dobj".to_string()],
        })
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("class hash mismatch"),
        "expected hash-mismatch error, got: {msg}"
    );
}

#[test]
fn test_execute_keeps_files_after_relayer_accepts() {
    let (entry, mut deps) = make_input_record("log_1.dobj");
    deps.relayer = Arc::new(MockRelayer {
        fail_wait: true,
        ..MockRelayer::default()
    });
    let paths = temp_paths();
    ensure_store_dirs(&paths).unwrap();
    write_object_file(&paths, &entry.record, &entry.file_name).unwrap();
    let driver = Driver::open(paths.clone(), deps).unwrap();

    let err = driver
        .execute(ExecuteActionInput {
            action: craft_basics("CraftWood"),
            input_objects: vec!["log_1.dobj".to_string()],
        })
        .unwrap_err();
    assert!(err.to_string().contains("relayer timeout"));

    // The relayer accepted the job but wait_for_tx_hash failed, so outputs
    // stay as Unknown (Pending requires a tx_hash). Files are kept so the
    // next sync_inventory can reconcile.
    let remaining = load_object_files(&paths).unwrap();
    assert_eq!(remaining.len(), 2);
    let input = remaining
        .iter()
        .find(|e| e.file_name == "log_1.dobj")
        .unwrap();
    assert!(!input.record.is_nullified());
    let output = remaining
        .iter()
        .find(|e| e.file_name != "log_1.dobj")
        .unwrap();
    assert_eq!(output.record.status, ObjectStatus::Unknown);
}

// ---------------------------------------------------------------------------
// import_object
// ---------------------------------------------------------------------------

/// Build a real Log object and return its serialized `.dobj` JSON plus the
/// source-tx hash and nullifier, so a test can configure `MockSynchronizer`
/// membership to drive `import_object` down each path.
fn make_importable_log() -> (String, Hash, Hash) {
    ensure_extra_pod_deserializers_registered();
    let catalog = make_catalog();
    let outputs = catalog
        .execute_action(craft_basics("FindLog"), dummy_grounding_witness(), vec![])
        .unwrap();
    let spendable = outputs.obj(0);
    let id = format!("{:#}", spendable.obj.commitment());
    let nullifier = txlib::object_nullifier_hash(&spendable.obj).unwrap();
    let record = ObjectRecord {
        id,
        class: craft_basics("Log"),
        status: ObjectStatus::Live,
        tx_hash: None,
        obj: spendable.obj,
        evidence: spendable.evidence,
    };
    let source_tx = record.evidence.tx_final;
    let json = serde_json::to_string(&record).unwrap();
    (json, source_tx, nullifier)
}

fn import_driver(
    grounded: HashSet<Hash>,
    nullifiers: HashSet<Hash>,
    fail_membership: bool,
) -> Driver {
    let paths = temp_paths();
    ensure_store_dirs(&paths).unwrap();
    let deps = DriverDeps {
        catalog: Arc::new(make_catalog()),
        synchronizer: Arc::new(MockSynchronizer {
            membership_grounded: grounded,
            membership_nullifiers: nullifiers,
            fail_membership,
            ..MockSynchronizer::default()
        }),
        relayer: Arc::new(MockRelayer::default()),
        payload_builder: Arc::new(MockPayloadBuilder),
    };
    Driver::open(paths, deps).unwrap()
}

#[test]
fn test_import_grounded_object_is_live() {
    let (json, source_tx, _nullifier) = make_importable_log();
    let driver = import_driver(HashSet::from([source_tx]), HashSet::new(), false);
    let summary = driver.import_object(&json).unwrap();
    assert_eq!(summary.class, craft_basics("Log"));
    assert_eq!(summary.status, ObjectStatus::Live);
    assert!(summary.file_name.starts_with("craft-basics__log_"));
}

#[test]
fn test_import_ungrounded_object_is_unknown() {
    let (json, _source_tx, _nullifier) = make_importable_log();
    let driver = import_driver(HashSet::new(), HashSet::new(), false);
    let summary = driver.import_object(&json).unwrap();
    assert_eq!(summary.status, ObjectStatus::Unknown);
}

#[test]
fn test_import_spent_object_is_rejected() {
    let (json, source_tx, nullifier) = make_importable_log();
    let driver = import_driver(
        HashSet::from([source_tx]),
        HashSet::from([nullifier]),
        false,
    );
    let err = driver.import_object(&json).unwrap_err();
    assert!(
        err.to_string().contains("spent"),
        "expected already-spent error, got: {err}"
    );
}

#[test]
fn test_import_duplicate_is_rejected() {
    let (json, source_tx, _nullifier) = make_importable_log();
    let driver = import_driver(HashSet::from([source_tx]), HashSet::new(), false);
    driver.import_object(&json).unwrap();
    let err = driver.import_object(&json).unwrap_err();
    assert!(
        err.to_string().contains("already in inventory"),
        "expected duplicate error, got: {err}"
    );
}

#[test]
fn test_import_sync_unreachable_falls_back_to_unknown() {
    let (json, _source_tx, _nullifier) = make_importable_log();
    let driver = import_driver(HashSet::new(), HashSet::new(), true);
    let summary = driver.import_object(&json).unwrap();
    assert_eq!(summary.status, ObjectStatus::Unknown);
}

#[test]
fn test_import_unknown_class_is_rejected() {
    let (json, _source_tx, _nullifier) = make_importable_log();
    let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
    value["class"]["name"] = serde_json::Value::String("Diamond".to_string());
    let tampered = serde_json::to_string(&value).unwrap();
    let driver = import_driver(HashSet::new(), HashSet::new(), false);
    let err = driver.import_object(&tampered).unwrap_err();
    assert!(
        err.to_string().contains("unknown class"),
        "expected unknown-class error, got: {err}"
    );
}

#[test]
fn test_import_class_hash_mismatch_is_rejected() {
    // Forge a real Log's class to craft-basics::Wood: the qualified name is a
    // known class, but the pod's `type` hash is IsLog, so the cryptographic
    // hash check must fire — the same guarantee execute relies on.
    let (json, _source_tx, _nullifier) = make_importable_log();
    let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
    value["class"]["name"] = serde_json::Value::String("Wood".to_string());
    let tampered = serde_json::to_string(&value).unwrap();
    let driver = import_driver(HashSet::new(), HashSet::new(), false);
    let err = driver.import_object(&tampered).unwrap_err();
    assert!(
        err.to_string().contains("class hash mismatch"),
        "expected hash-mismatch error, got: {err}"
    );
}
