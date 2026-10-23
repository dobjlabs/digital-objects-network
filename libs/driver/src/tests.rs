use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::RwLock;

use anyhow::{Result, anyhow};
use payload::test_state::TestState;
use pod2::middleware::Hash;
use sdk::SpendableObjects;
use tempfile::tempdir;
use txlib::{GroundingWitness, StateHeader};

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
use crate::{ActionQuery, DriverPaths, ExecuteActionInput, ObjectQuery};
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
        StateHeader::new(
            1,
            1,
            pod2::middleware::EMPTY_HASH,
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

fn apply_tx(state: &mut TestState, tx: &txlib::Tx) {
    state.apply_tx(
        tx.created_commitments().unwrap(),
        tx.nullifier_hashes().unwrap(),
    );
}

fn state_header(state: &TestState) -> StateHeader {
    let (created_root, nullifiers_root, prior_state_history_root) = state.roots();
    StateHeader::new(
        state.block_number,
        state.block_timestamp,
        state.block_hash,
        created_root,
        nullifiers_root,
        prior_state_history_root,
    )
}

fn craft_basics(name: &str) -> QualifiedName {
    QualifiedName::new("craft-basics", name)
}

fn make_input_record(file_name: &str) -> (ObjectFileEntry, DriverDeps) {
    ensure_extra_pod_deserializers_registered();
    let catalog = make_catalog();
    let outputs = catalog
        .execute_action(
            craft_basics("FindLog"),
            dummy_grounding_witness(),
            vec![],
            &Default::default(),
        )
        .unwrap();
    let source_tx = outputs.tx.clone();
    let spendable = outputs.obj(0);
    let content_hash = spendable.obj.commitment();
    let record = ObjectRecord {
        content_hash,
        class: craft_basics("Log"),
        status: ObjectStatus::Live,
        tx_hash: None,
        obj: spendable.obj,
        tx_final: tx_hash(&source_tx),
    };
    let mut state = TestState::default();
    apply_tx(&mut state, &source_tx);
    (
        ObjectFileEntry {
            file_name: file_name.to_string(),
            record,
        },
        DriverDeps {
            catalog: Arc::new(RwLock::new(Arc::new(catalog))),
            synchronizer: Arc::new(MockSynchronizer {
                state,
                ..MockSynchronizer::default()
            }),
            relayer: Arc::new(MockRelayer::default()),
            payload_builder: Arc::new(MockPayloadBuilder),
        },
    )
}

#[test]
fn payload_commits_to_intermediate_and_deleted_states() {
    use payload::payload::{Payload, PayloadProof};
    use pod2::{
        backends::plonky2::mock::mainpod::MockProver,
        frontend::MultiPodBuilder,
        middleware::{Params, Predicate, StrKey, VDSet, Value},
    };
    use pod2utils::{dict, macros::BuildContext, map, op, set};

    let params = Params::default();
    let txlib = Arc::new(txlib::predicates::module());
    // An unrestricted class isolates replay and payload binding from application rules.
    let guard_module = Arc::new(
        pod2::lang::load_module(
            "PayloadTestGuard(obj, state_header, chain_start, chain_end) = AND(Equal(obj, obj))",
            "payload_test",
            &params,
            &[],
        )
        .unwrap(),
    );
    let guard_type = Value::from(
        Predicate::Custom(
            guard_module
                .predicate_ref_by_name("PayloadTestGuard")
                .unwrap(),
        )
        .hash(),
    );
    let mut ctx = BuildContext::new(
        MultiPodBuilder::new(&params, &VDSet::new(&[])),
        vec![
            Arc::new(txlib::predicates::events_module()),
            txlib.clone(),
            guard_module,
        ],
    );
    let mut builder = txlib::TxBuilder::new(&mut ctx, &[], Arc::new(dummy_grounding_witness()));
    let chain_start = builder.chain_position();
    let scope = builder.begin_action();
    let initial = dict!({"type" => guard_type, "key" => 1_i64, "version" => 0_i64});
    let (first, _, insert_handle) = builder.insert(&mut ctx, &initial);
    let mut second = first.clone();
    second
        .update(&StrKey::from("version"), &Value::from(1_i64))
        .unwrap();
    txlib::rekey(&mut second);
    let (_, mutate_handle) = builder.mutate(&mut ctx, &second, &first);
    let (_, delete_handle) = builder.delete(&mut ctx, &second);
    for (object, handle) in [
        (&first, insert_handle),
        (&second, mutate_handle),
        (&second, delete_handle),
    ] {
        let equal = ctx.builder.priv_op(op!(Equal(object, object))).unwrap();
        let guard = ctx
            .apply_custom_pred(
                false,
                "PayloadTestGuard",
                map!({
                    "state_header" => builder.state_header().array(),
                    "chain_start" => chain_start,
                    "chain_end" => builder.chain_position()
                }),
                vec![equal],
            )
            .unwrap();
        builder.set_guard(handle, guard);
    }
    builder.end_action(scope);
    let (statement, tx, _) = builder.finalize(&mut ctx).unwrap();
    ctx.builder.reveal(&statement).unwrap();
    let pod = ctx
        .builder
        .solve()
        .unwrap()
        .prove(&MockProver {})
        .unwrap()
        .output_pod()
        .clone();
    pod.pod.verify().unwrap();

    let payload =
        Payload::from_tx(PayloadProof::empty_for_test(), &tx, tx.state_header.hash()).unwrap();
    let expected_created = set!(first, second);
    assert_eq!(tx.created.commitment(), expected_created.commitment());
    assert_eq!(payload.created.len(), 2);
    assert_eq!(payload.nullifiers.len(), 2);
    let predicate = txlib.predicate_ref_by_name("TxFinalized").unwrap();
    assert_eq!(
        pod.public_statements,
        vec![payload.finalized_statement(predicate.clone())]
    );
    let mut missing_intermediates = payload.clone();
    missing_intermediates.created = Vec::new();
    assert_ne!(
        missing_intermediates.finalized_statement(predicate),
        pod.public_statements[0]
    );

    let mut state = TestState::default();
    apply_tx(&mut state, &tx);
    state.created_membership_proof(first.commitment());
    state.created_membership_proof(second.commitment());
}

struct MockPayloadBuilder;

impl PayloadBuilder for MockPayloadBuilder {
    fn build_payload(
        &self,
        _old_state_root: &Hash,
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
            current_state_root: state_header(&self.state).hash(),
        })
    }

    fn fetch_grounding_witness(
        &self,
        _sync_api_url: &str,
        object_commitments: &[Hash],
    ) -> Result<GroundingWitness> {
        let created_proofs = object_commitments
            .iter()
            .copied()
            .map(|commitment| (commitment, self.state.created_membership_proof(commitment)))
            .collect::<HashMap<_, _>>();
        Ok(GroundingWitness::new(
            state_header(&self.state),
            created_proofs,
        ))
    }

    fn fetch_membership_with_nullifiers(
        &self,
        _sync_api_url: &str,
        _object_commitments: &[Hash],
        _nullifiers: &[Hash],
    ) -> Result<SynchronizerMembership> {
        if self.fail_membership {
            return Err(anyhow!("synchronizer unreachable"));
        }
        Ok(SynchronizerMembership {
            created_objects: self.membership_grounded.clone(),
            on_chain_nullifiers: self.membership_nullifiers.clone(),
        })
    }

    fn wait_for_tx(
        &self,
        _sync_api_url: &str,
        _created_commitments: &[Hash],
        _nullifiers: &[Hash],
        _timeout_secs: u64,
        _poll_interval_ms: u64,
    ) -> Result<SynchronizerHead> {
        if self.fail_wait {
            return Err(anyhow!("synchronizer timeout"));
        }
        Ok(SynchronizerHead {
            current_state_root: state_header(&self.state).hash(),
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
            tx_final: Default::default(),
            state_root: Default::default(),
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

    fn lookup_tx_hashes(
        &self,
        _relayer_api_url: &str,
        tx_finals: &[Hash],
    ) -> Result<HashMap<Hash, String>> {
        Ok(tx_finals.iter().map(|h| (*h, "0xtx".to_string())).collect())
    }
}

#[test]
fn test_list_actions_filters_by_input_class() {
    // Use the in-memory test catalog so the assertion does not depend on
    // whatever pexe plugins happen to live under ~/.dobj/actions/.
    let paths = temp_paths();
    ensure_store_dirs(&paths).unwrap();
    let deps = DriverDeps {
        catalog: Arc::new(RwLock::new(Arc::new(make_catalog()))),
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
fn test_confirmed_actions_update_input_and_output_liveness() {
    for action_name in ["CraftWood", "RekeyLog"] {
        let (mut entry, deps) = make_input_record("log_1.dobj");
        entry.record.tx_hash = Some("0xcreation".to_string());
        let paths = temp_paths();
        write_object_file(&paths, &entry.record, &entry.file_name).unwrap();
        let driver = Driver::open(paths.clone(), deps).unwrap();
        let action = craft_basics(action_name);
        assert!(driver.check_action(&action).unwrap().feasible);

        let result = driver
            .execute(ExecuteActionInput {
                action: action.clone(),
                input_objects: vec![entry.file_name.clone()],
                args: Default::default(),
            })
            .unwrap();

        let report = driver.check_action(&action).unwrap();
        assert!(
            report
                .available_inputs
                .iter()
                .all(|candidate| candidate.file_name != entry.file_name),
            "{action_name} suggested its consumed input: {report:?}"
        );
        assert_eq!(report.feasible, action_name == "RekeyLog");
        if action_name == "RekeyLog" {
            assert_eq!(report.available_inputs.len(), 1);
            assert!(
                result
                    .output_files
                    .contains(&report.available_inputs[0].file_name)
            );
        }

        assert_eq!(result.nullified_files, vec![entry.file_name.clone()]);
        assert!(!paths.objects_dir.join(&entry.file_name).exists());
        let consumed = crate::object_record::parse_object_record_file(
            &paths.nullified_objects_dir.join(&entry.file_name),
        )
        .unwrap();
        assert_eq!(consumed.status, ObjectStatus::Nullified);
        assert_eq!(consumed.tx_hash, entry.record.tx_hash);
        assert_eq!(consumed.tx_final, entry.record.tx_final);
        assert_eq!(consumed.content_hash, entry.record.content_hash);
        for file_name in &result.output_files {
            let output =
                crate::object_record::parse_object_record_file(&paths.objects_dir.join(file_name))
                    .unwrap();
            assert_eq!(output.status, ObjectStatus::Live);
            assert_eq!(output.tx_hash.as_deref(), Some("0xtx"));
        }
    }
}

#[test]
fn test_execute_keeps_inputs_live_without_synchronizer_confirmation() {
    let (entry, mut deps) = make_input_record("log_1.dobj");
    let mut state = TestState::default();
    state.apply_tx(vec![entry.record.obj.commitment()], vec![]);
    deps.synchronizer = Arc::new(MockSynchronizer {
        state,
        fail_wait: true,
        ..MockSynchronizer::default()
    });
    let paths = temp_paths();
    write_object_file(&paths, &entry.record, &entry.file_name).unwrap();
    let driver = Driver::open(paths.clone(), deps).unwrap();
    let action = craft_basics("CraftWood");

    let error = driver
        .execute(ExecuteActionInput {
            action: action.clone(),
            input_objects: vec![entry.file_name.clone()],
            args: Default::default(),
        })
        .unwrap_err();
    assert!(error.to_string().contains("synchronizer timeout"));

    let report = driver.check_action(&action).unwrap();
    assert!(report.feasible);
    assert_eq!(report.available_inputs[0].file_name, entry.file_name);
    assert!(!paths.nullified_objects_dir.join(&entry.file_name).exists());
    let records = load_object_files(&paths).unwrap();
    assert_eq!(records.len(), 2);
    for stored in records {
        if stored.file_name == entry.file_name {
            assert_eq!(stored.record.status, ObjectStatus::Live);
        } else {
            assert_eq!(stored.record.status, ObjectStatus::Unknown);
            assert_eq!(stored.record.tx_hash.as_deref(), Some("0xtx"));
        }
    }
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
            args: Default::default(),
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
            args: Default::default(),
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
            args: Default::default(),
        })
        .unwrap_err();
    assert!(err.to_string().contains("relayer timeout"));

    // The relayer accepted the job but wait_for_tx_hash failed, so outputs
    // stay as Unknown (Pending requires a tx_hash). Files are kept so the
    // next sync_objects can reconcile.
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
        .execute_action(
            craft_basics("FindLog"),
            dummy_grounding_witness(),
            vec![],
            &Default::default(),
        )
        .unwrap();
    let spendable = outputs.obj(0);
    let content_hash = spendable.obj.commitment();
    let commitment = spendable.obj.commitment();
    let nullifier = txlib::object_nullifier_hash(&spendable.obj).unwrap();
    let record = ObjectRecord {
        content_hash,
        class: craft_basics("Log"),
        status: ObjectStatus::Live,
        tx_hash: None,
        obj: spendable.obj,
        tx_final: outputs.tx.dict().commitment(),
    };
    let json = serde_json::to_string(&record).unwrap();
    (json, commitment, nullifier)
}

fn import_driver(
    grounded: HashSet<Hash>,
    nullifiers: HashSet<Hash>,
    fail_membership: bool,
) -> Driver {
    let paths = temp_paths();
    ensure_store_dirs(&paths).unwrap();
    let deps = DriverDeps {
        catalog: Arc::new(RwLock::new(Arc::new(make_catalog()))),
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
fn test_read_object_raw_returns_stored_file() {
    let (json, source_tx, _nullifier) = make_importable_log();
    let driver = import_driver(HashSet::from([source_tx]), HashSet::new(), false);
    let summary = driver.import_object(&json).unwrap();
    let raw = driver
        .read_object_raw(std::path::Path::new(&summary.file_name))
        .unwrap();
    let on_disk =
        std::fs::read_to_string(driver.paths().objects_dir.join(&summary.file_name)).unwrap();
    assert_eq!(raw, on_disk);
}

#[test]
fn test_read_object_raw_missing_file_is_not_found() {
    let driver = import_driver(HashSet::new(), HashSet::new(), false);
    let err = driver
        .read_object_raw(std::path::Path::new("missing.dobj"))
        .unwrap_err();
    assert!(matches!(
        err.downcast_ref::<crate::error::DriverError>(),
        Some(crate::error::DriverError::ObjectFileNotFound(_))
    ));
}

#[test]
fn test_discard_object_unsafe_moves_file_out_of_store() {
    let (json, _source_tx, _nullifier) = make_importable_log();
    let driver = import_driver(HashSet::new(), HashSet::new(), false);
    let record: ObjectRecord = serde_json::from_str(&json).unwrap();
    let record = ObjectRecord {
        status: ObjectStatus::Pending,
        ..record
    };
    write_object_file(driver.paths(), &record, "stale.dobj").unwrap();
    let on_disk = std::fs::read_to_string(driver.paths().objects_dir.join("stale.dobj")).unwrap();

    let summary = driver
        .discard_object_unsafe(std::path::Path::new("stale.dobj"))
        .unwrap();
    assert_eq!(summary.status, ObjectStatus::Pending);
    assert!(driver.list_objects(None).unwrap().is_empty());
    let discarded =
        std::fs::read_to_string(driver.paths().discarded_objects_dir.join("stale.dobj")).unwrap();
    assert_eq!(discarded, on_disk);

    let spent = ObjectRecord {
        status: ObjectStatus::Nullified,
        ..record
    };
    write_object_file(driver.paths(), &spent, "spent.dobj").unwrap();
    for file_name in ["stale.dobj", "spent.dobj"] {
        let err = driver
            .discard_object_unsafe(std::path::Path::new(file_name))
            .unwrap_err();
        assert!(matches!(
            err.downcast_ref::<crate::error::DriverError>(),
            Some(crate::error::DriverError::ObjectFileNotFound(_))
        ));
    }
    assert!(
        driver
            .paths()
            .nullified_objects_dir
            .join("spent.dobj")
            .exists()
    );
}

#[test]
fn test_list_objects_can_exclude_nullified() {
    let (json, _source_tx, _nullifier) = make_importable_log();
    let driver = import_driver(HashSet::new(), HashSet::new(), false);
    let live_record: ObjectRecord = serde_json::from_str(&json).unwrap();
    let nullified_record = ObjectRecord {
        status: ObjectStatus::Nullified,
        ..live_record.clone()
    };
    write_object_file(driver.paths(), &live_record, "live.dobj").unwrap();
    write_object_file(driver.paths(), &nullified_record, "spent.dobj").unwrap();

    let all = driver.list_objects(None).unwrap();
    assert_eq!(all.len(), 2);

    let query = ObjectQuery {
        exclude_nullified: true,
        ..ObjectQuery::default()
    };
    let unspent = driver.list_objects(Some(&query)).unwrap();
    let file_names: Vec<&str> = unspent.iter().map(|o| o.file_name.as_str()).collect();
    assert_eq!(file_names, vec!["live.dobj"]);
}

/// A driver with an empty catalog and mock chain deps. `install_plugin` only
/// touches the actions dir and the catalog, so the mocks are never exercised.
fn empty_catalog_driver() -> Driver {
    let paths = temp_paths();
    ensure_store_dirs(&paths).unwrap();
    let empty =
        PexeCatalog::from_bytes(std::iter::empty::<(std::path::PathBuf, Vec<u8>)>(), true).unwrap();
    let deps = DriverDeps {
        catalog: Arc::new(RwLock::new(Arc::new(empty))),
        synchronizer: Arc::new(MockSynchronizer::default()),
        relayer: Arc::new(MockRelayer::default()),
        payload_builder: Arc::new(MockPayloadBuilder),
    };
    Driver::open(paths, deps).unwrap()
}

#[test]
fn install_plugin_writes_and_hot_reloads() {
    let driver = empty_catalog_driver();
    assert!(driver.list_actions(None).unwrap().is_empty());

    let name = driver.install_plugin(&test_plugin_bytes()).unwrap();
    assert_eq!(name, "craft-basics");
    // Live without a restart...
    assert!(!driver.list_actions(None).unwrap().is_empty());
    // ...and the archive landed in the actions dir.
    assert!(
        driver
            .paths()
            .actions_dir
            .join("craft-basics.pexe")
            .exists()
    );
}

#[test]
fn install_plugin_rejects_bad_archive_and_leaves_catalog_intact() {
    let driver = empty_catalog_driver();
    let err = driver.install_plugin(b"not a valid pexe").unwrap_err();
    assert!(
        err.to_string().contains("archive") || err.to_string().contains("pexe"),
        "expected an archive error, got: {err}"
    );
    // A rejected install must not populate the catalog or leave a file behind.
    assert!(driver.list_actions(None).unwrap().is_empty());
    let leftovers = std::fs::read_dir(&driver.paths().actions_dir)
        .map(|rd| rd.filter_map(|e| e.ok()).count())
        .unwrap_or(0);
    assert_eq!(leftovers, 0, "a rejected install left files behind");
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
        err.to_string().contains("already present"),
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
