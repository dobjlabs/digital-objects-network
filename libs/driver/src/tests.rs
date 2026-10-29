use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};

use anyhow::{Result, anyhow};
use payload::test_state::TestState;
use payload::{decode_hash_hex, encode_hash_hex};
use pod2::middleware::Hash;
use sdk::SpendableObjects;
use tempfile::tempdir;
use txlib::{GroundingWitness, MAX_STATE_ROOT_AGE_BLOCKS, StateHeader};
use wire_types::relayer::{JobStatus, JobStatusResponse, SubmitProofResponse};
use wire_types::transactions::{TransactionRecord, TransactionState};

use crate::catalog::ActionCatalog;
use crate::clients::{
    RelayerClient, SynchronizerClient, SynchronizerHead, SynchronizerMembership,
    SynchronizerTxStatus,
};
use crate::driver::{Driver, DriverDeps, PayloadBuilder};
use crate::object_record::{FileStatus, ObjectRecord, ensure_extra_pod_deserializers_registered};
use crate::object_store::{
    ensure_store_dirs, load_object_files, object_file_name, write_object_file,
};
use crate::pexe_catalog::{PexeCatalog, test_plugin_bytes};
use crate::{ActionQuery, DriverPaths, ExecuteActionInput, NoopExecutionReporter, ObjectQuery};
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

/// A local Log with shared mock chain and relayer state.
struct Fixture {
    paths: DriverPaths,
    driver: Driver,
    input_file: String,
    chain: Arc<Mutex<MockChain>>,
    relayer: Arc<Mutex<MockRelayerState>>,
}

fn fixture(file_name: &str) -> Fixture {
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
    let mut state = TestState::default();
    apply_tx(&mut state, &source_tx);
    fixture_holding(
        catalog,
        state,
        file_name,
        craft_basics("Log"),
        outputs.obj(0),
        tx_hash(&source_tx),
    )
}

/// A Log filed under its generated name, which settlement keeps when the
/// Log is spent.
fn generated_fixture() -> Fixture {
    let mut fx = fixture("log_1.dobj");
    let log = fx.stored(&fx.input_file);
    let generated = object_file_name(&log.class, &log.obj).unwrap();
    fx.driver
        .rename_object(std::path::Path::new(&fx.input_file), &generated)
        .unwrap();
    fx.input_file = generated;
    fx
}

/// Run `action` outside the driver against `state`, then apply its
/// transaction there.
fn run_offline(
    catalog: &PexeCatalog,
    state: &mut TestState,
    action: &str,
    inputs: Vec<sdk::SpendableObject>,
) -> sdk::SpendableObjects {
    let created_proofs = inputs
        .iter()
        .map(|input| {
            let commitment = input.obj.commitment();
            (commitment, state.created_membership_proof(commitment))
        })
        .collect::<HashMap<_, _>>();
    let witness = GroundingWitness::new(state_header(state), created_proofs);
    let outputs = catalog
        .execute_action(craft_basics(action), witness, inputs, &Default::default())
        .unwrap();
    apply_tx(state, &outputs.tx);
    outputs
}

/// A local WoodPick, which `UseWoodPick` mutates, filed as `file_name`.
fn pick_fixture(file_name: &str) -> Fixture {
    ensure_extra_pod_deserializers_registered();
    let catalog = make_catalog();
    let mut state = TestState::default();
    let log_a = run_offline(&catalog, &mut state, "FindLog", vec![]).obj(0);
    let log_b = run_offline(&catalog, &mut state, "FindLog", vec![]).obj(0);
    let wood_a = run_offline(&catalog, &mut state, "CraftWood", vec![log_a]).obj(0);
    let wood_b = run_offline(&catalog, &mut state, "CraftWood", vec![log_b]).obj(0);
    let stick = run_offline(&catalog, &mut state, "CraftSticks", vec![wood_a]).obj(0);
    let crafted = run_offline(&catalog, &mut state, "CraftWoodPick", vec![wood_b, stick]);
    let tx_final = tx_hash(&crafted.tx);
    fixture_holding(
        catalog,
        state,
        file_name,
        craft_basics("WoodPick"),
        crafted.obj(0),
        tx_final,
    )
}

/// A driver whose store holds one live object, `spendable`, as `file_name`,
/// on a mock chain at `state`.
fn fixture_holding(
    catalog: PexeCatalog,
    state: TestState,
    file_name: &str,
    class: QualifiedName,
    spendable: sdk::SpendableObject,
    tx_final: Hash,
) -> Fixture {
    let record = ObjectRecord {
        content_hash: spendable.obj.commitment(),
        class,
        status: FileStatus::Live,
        tx_hash: None,
        obj: spendable.obj,
        tx_final,
    };
    let chain = Arc::new(Mutex::new(MockChain {
        head_block: state.block_number,
        ..MockChain::default()
    }));
    let relayer = Arc::new(Mutex::new(MockRelayerState::default()));
    let deps = DriverDeps {
        catalog: Arc::new(RwLock::new(Arc::new(catalog))),
        synchronizer: Arc::new(MockSynchronizer {
            state,
            chain: chain.clone(),
        }),
        relayer: Arc::new(MockRelayer {
            inner: relayer.clone(),
        }),
        payload_builder: Arc::new(MockPayloadBuilder),
    };
    let paths = temp_paths();
    write_object_file(&paths, &record, file_name).unwrap();
    let driver = Driver::open(paths.clone(), deps).unwrap();
    Fixture {
        paths,
        driver,
        input_file: file_name.to_string(),
        chain,
        relayer,
    }
}

impl Fixture {
    fn input(&self, action: &str) -> ExecuteActionInput {
        ExecuteActionInput {
            action: craft_basics(action),
            input_objects: vec![self.input_file.clone()],
            args: Default::default(),
        }
    }

    fn land(&self, record: &TransactionRecord) {
        self.land_with_tx_hash(record, "0xlanded");
    }

    /// Apply the transaction's effects to the chain and record it as applied by `tx_hash`.
    fn land_with_tx_hash(&self, record: &TransactionRecord, tx_hash: &str) {
        let mut chain = self.chain.lock().unwrap();
        for hex in &record.created {
            chain.created.insert(decode_hash_hex(hex).unwrap());
        }
        for hex in &record.nullifiers {
            chain.nullifiers.insert(decode_hash_hex(hex).unwrap());
        }
        let status = SynchronizerTxStatus {
            tx_hash: tx_hash.to_string(),
            block_number: chain.head_block,
            error: None,
        };
        chain.statuses.insert(tx_final_of(record), status);
    }

    /// Record the transaction's proof as rejected by the synchronizer, with no chain effects.
    fn reject(&self, record: &TransactionRecord, error: &str) {
        let mut chain = self.chain.lock().unwrap();
        let status = SynchronizerTxStatus {
            tx_hash: "0xrejected".to_string(),
            block_number: chain.head_block,
            error: Some(error.to_string()),
        };
        chain.statuses.insert(tx_final_of(record), status);
    }

    fn advance_head_to(&self, block: i64) {
        self.chain.lock().unwrap().head_block = block;
    }

    fn stored(&self, file_name: &str) -> ObjectRecord {
        let live = self.paths.objects_dir.join(file_name);
        let path = if live.exists() {
            live
        } else {
            self.paths.nullified_objects_dir.join(file_name)
        };
        crate::object_record::parse_object_record_file(&path).unwrap()
    }

    fn summary(&self, file_name: &str) -> wire_types::ObjectSummary {
        self.driver
            .read_object(std::path::Path::new(file_name))
            .unwrap()
    }
}

struct SubmissionGate {
    entered: std::sync::mpsc::Receiver<()>,
    release: std::sync::mpsc::Sender<()>,
}

impl SubmissionGate {
    fn wait(&self) {
        self.entered
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
    }
}

impl Drop for SubmissionGate {
    fn drop(&mut self) {
        let _ = self.release.send(());
    }
}

impl Fixture {
    fn pause_submission(&self, fail: bool) -> SubmissionGate {
        let (entered, notification) = std::sync::mpsc::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        self.relayer.lock().unwrap().on_submit = Some(Box::new(move || {
            entered.send(()).unwrap();
            blocked
                .recv_timeout(std::time::Duration::from_secs(5))
                .map_err(|err| anyhow!("submission was not released: {err}"))?;
            if fail {
                return Err(anyhow!("blocked submission failed"));
            }
            Ok(())
        }));
        SubmissionGate {
            entered: notification,
            release,
        }
    }
}

fn tx_final_of(record: &TransactionRecord) -> Hash {
    crate::transactions::record_tx_final(record).unwrap()
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
struct MockChain {
    created: HashSet<Hash>,
    nullifiers: HashSet<Hash>,
    /// Transactions carried by a committed blob, applied or rejected.
    statuses: HashMap<Hash, SynchronizerTxStatus>,
    head_block: i64,
    fail_membership: bool,
    fail_head: bool,
    fail_status: bool,
    head_requests: usize,
    queried_statuses: Vec<Hash>,
    on_head: Option<Box<dyn FnOnce() + Send>>,
    on_status: Option<Box<dyn FnOnce() + Send>>,
}

struct MockSynchronizer {
    /// Source of grounding witnesses; status and membership reads use `chain` instead.
    state: TestState,
    chain: Arc<Mutex<MockChain>>,
}

impl SynchronizerClient for MockSynchronizer {
    fn fetch_head(&self, _sync_api_url: &str) -> Result<SynchronizerHead> {
        let mut chain = self.chain.lock().unwrap();
        chain.head_requests += 1;
        if chain.fail_head {
            return Err(anyhow!("synchronizer head unavailable"));
        }
        let head = SynchronizerHead {
            state_root: state_header(&self.state).hash(),
            block_number: chain.head_block,
        };
        let on_head = chain.on_head.take();
        drop(chain);
        if let Some(on_head) = on_head {
            on_head();
        }
        Ok(head)
    }

    fn fetch_tx_status(
        &self,
        _sync_api_url: &str,
        tx_final: &Hash,
    ) -> Result<Option<SynchronizerTxStatus>> {
        let mut chain = self.chain.lock().unwrap();
        chain.queried_statuses.push(*tx_final);
        if chain.fail_status {
            return Err(anyhow!("synchronizer unreachable"));
        }
        let status = chain.statuses.get(tx_final).cloned();
        let on_status = chain.on_status.take();
        drop(chain);
        if let Some(on_status) = on_status {
            on_status();
        }
        Ok(status)
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
        object_commitments: &[Hash],
        nullifiers: &[Hash],
    ) -> Result<SynchronizerMembership> {
        let chain = self.chain.lock().unwrap();
        if chain.fail_membership {
            return Err(anyhow!("synchronizer unreachable"));
        }
        let membership = SynchronizerMembership {
            created_objects: object_commitments
                .iter()
                .filter(|commitment| chain.created.contains(commitment))
                .copied()
                .collect(),
            on_chain_nullifiers: nullifiers
                .iter()
                .filter(|nullifier| chain.nullifiers.contains(nullifier))
                .copied()
                .collect(),
            block_number: chain.head_block,
        };
        Ok(membership)
    }
}

#[derive(Default)]
struct MockRelayerState {
    fail_submit: bool,
    submit_calls: usize,
    on_submit: Option<Box<dyn FnOnce() -> Result<()> + Send>>,
    submitted: Vec<Vec<u8>>,
    status: Option<JobStatus>,
    tx_hash: Option<String>,
    last_error: Option<String>,
}

struct MockRelayer {
    inner: Arc<Mutex<MockRelayerState>>,
}

impl RelayerClient for MockRelayer {
    fn submit_proof(
        &self,
        _relayer_api_url: &str,
        payload_bytes: &[u8],
        _client_ref: Option<String>,
    ) -> Result<SubmitProofResponse> {
        let on_submit = {
            let mut inner = self.inner.lock().unwrap();
            inner.submit_calls += 1;
            inner.on_submit.take()
        };
        if let Some(on_submit) = on_submit {
            on_submit()?;
        }
        let mut inner = self.inner.lock().unwrap();
        if inner.fail_submit {
            return Err(anyhow!("relayer submit failed"));
        }
        inner.submitted.push(payload_bytes.to_vec());
        Ok(SubmitProofResponse {
            job_id: format!("job-{}", inner.submitted.len()),
            status: JobStatus::Queued,
            tx_final: Default::default(),
            state_root: Default::default(),
            attempt_count: 0,
            created_at: 0,
        })
    }

    fn fetch_job_status(&self, _relayer_api_url: &str, job_id: &str) -> Result<JobStatusResponse> {
        let inner = self.inner.lock().unwrap();
        Ok(JobStatusResponse {
            job_id: job_id.to_string(),
            status: inner.status.unwrap_or(JobStatus::Queued),
            tx_hash: inner.tx_hash.clone(),
            block_number: None,
            attempt_count: 1,
            last_error: inner.last_error.clone(),
            updated_at: 0,
            created_at: 0,
            tx_final: Default::default(),
            state_root: Default::default(),
        })
    }
}

fn mock_deps() -> (DriverDeps, Arc<Mutex<MockChain>>) {
    let chain = Arc::new(Mutex::new(MockChain::default()));
    let deps = DriverDeps {
        catalog: Arc::new(RwLock::new(Arc::new(make_catalog()))),
        synchronizer: Arc::new(MockSynchronizer {
            state: TestState::default(),
            chain: chain.clone(),
        }),
        relayer: Arc::new(MockRelayer {
            inner: Arc::new(Mutex::new(MockRelayerState::default())),
        }),
        payload_builder: Arc::new(MockPayloadBuilder),
    };
    (deps, chain)
}

#[test]
fn test_list_actions_filters_by_input_class() {
    // Use the in-memory test catalog so the assertion does not depend on
    // whatever pexe plugins happen to live under ~/.dobj/actions/.
    let paths = temp_paths();
    ensure_store_dirs(&paths).unwrap();
    let (deps, _) = mock_deps();
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
fn batch_summaries_skip_unreadable_files_and_include_transaction_ownership() {
    let fx = fixture("log_1.dobj");
    let record = fx.driver.execute(fx.input("CraftWood")).unwrap();
    std::fs::write(fx.paths.objects_dir.join("invalid.dobj"), "not JSON").unwrap();
    let paths = [
        fx.input_file.clone(),
        "missing.dobj".to_string(),
        "invalid.dobj".to_string(),
        record.output_files[0].clone(),
    ];
    let summaries = fx.driver.read_available_objects(&paths).unwrap();
    assert_eq!(summaries.len(), 2);
    let input = &summaries[0];
    assert_eq!(input.file_name, paths[0]);
    assert_eq!(input.held_by_run_id.as_deref(), Some("execute"));
    let output = &summaries[1];
    assert_eq!(output.status, ObjectStatus::Pending);
    assert_eq!(
        serde_json::to_value(output).unwrap(),
        serde_json::to_value(fx.summary(&paths[3])).unwrap()
    );
    std::fs::rename(
        &fx.paths.transactions_dir,
        fx.paths.transactions_dir.with_extension("saved"),
    )
    .unwrap();
    std::fs::write(&fx.paths.transactions_dir, "not a directory").unwrap();
    assert!(fx.driver.read_available_objects(&paths).is_err());
}

#[test]
fn execute_records_and_submits_and_settle_lands() {
    for action_name in ["CraftWood", "RekeyLog"] {
        let fx = generated_fixture();
        let action = craft_basics(action_name);
        assert!(fx.driver.check_action(&action).unwrap().feasible);

        let result = fx.driver.execute(fx.input(action_name)).unwrap();
        assert_eq!(result.input_files, vec![fx.input_file.clone()]);
        assert_eq!(result.owner, "execute");
        assert_eq!(fx.relayer.lock().unwrap().submitted, vec![vec![1, 2, 3]]);

        let record = fx.driver.get_transaction(&tx_final_of(&result)).unwrap();
        assert_eq!(record, result);
        assert_eq!(record.state, TransactionState::Submitted);
        assert_eq!(record.relayer_job_id.as_deref(), Some("job-1"));
        assert!(record.submit_attempted_at_ms.is_some());

        assert_eq!(fx.stored(&fx.input_file).status, FileStatus::Live);
        assert_eq!(
            fx.summary(&fx.input_file).held_by_run_id.as_deref(),
            Some("execute")
        );
        let report = fx.driver.check_action(&action).unwrap();
        assert!(
            report
                .available_inputs
                .iter()
                .all(|candidate| candidate.file_name != fx.input_file),
            "{action_name} offered its locked input: {report:?}"
        );
        assert!(!report.feasible);
        for file_name in &result.output_files {
            assert_eq!(fx.stored(file_name).status, FileStatus::Unknown);
            let summary = fx.summary(file_name);
            assert_eq!(summary.status, ObjectStatus::Pending);
            assert_eq!(summary.tx_hash, None);
        }
        let err = fx
            .driver
            .prove(fx.input(action_name), "second", &NoopExecutionReporter)
            .unwrap_err();
        assert!(err.to_string().contains("awaiting the chain"), "{err}");

        fx.land_with_tx_hash(&record, "0xtx");
        let outcome = fx.driver.settle().unwrap();
        assert_eq!(outcome.records.len(), 1);
        assert_eq!(outcome.records[0].state, TransactionState::Landed);
        assert_eq!(outcome.records[0].tx_hash.as_deref(), Some("0xtx"));
        let mut changed = outcome
            .objects
            .iter()
            .map(|summary| summary.file_name.clone())
            .collect::<Vec<_>>();
        changed.sort();
        let mut expected = result.output_files.clone();
        expected.push(fx.input_file.clone());
        expected.sort();
        assert_eq!(changed, expected);

        assert!(!fx.paths.objects_dir.join(&fx.input_file).exists());
        let consumed = fx.stored(&fx.input_file);
        assert_eq!(consumed.status, FileStatus::Nullified);
        assert_eq!(fx.summary(&fx.input_file).held_by_run_id, None);
        for file_name in &result.output_files {
            assert_eq!(fx.stored(file_name).status, FileStatus::Live);
            let summary = fx.summary(file_name);
            assert_eq!(summary.status, ObjectStatus::Live);
            assert_eq!(summary.tx_hash.as_deref(), Some("0xtx"));
        }
        let report = fx.driver.check_action(&action).unwrap();
        assert_eq!(report.feasible, action_name == "RekeyLog");
        if action_name == "RekeyLog" {
            assert!(
                result
                    .output_files
                    .contains(&report.available_inputs[0].file_name)
            );
        }
        let outcome = fx.driver.settle().unwrap();
        assert!(outcome.records.is_empty() && outcome.objects.is_empty());
    }
}

#[test]
fn execute_keeps_the_record_when_the_relayer_refuses() {
    let fx = fixture("log_1.dobj");
    fx.relayer.lock().unwrap().fail_submit = true;

    let err = fx.driver.execute(fx.input("CraftWood")).unwrap_err();
    assert!(err.to_string().contains("relayer submit failed"), "{err}");

    let records = fx.driver.list_transactions().unwrap();
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record.state, TransactionState::Recorded);
    assert!(record.submit_attempted_at_ms.is_some());
    assert_eq!(record.relayer_job_id, None);
    for file_name in &record.output_files {
        assert_eq!(fx.summary(file_name).status, ObjectStatus::Unknown);
    }
    assert_eq!(fx.stored(&fx.input_file).status, FileStatus::Live);

    // A failed request may have reached the relayer; keep the input locked.
    let err = fx
        .driver
        .delete_transaction(&tx_final_of(record))
        .unwrap_err();
    assert!(err.to_string().contains("may still land"), "{err}");
    let err = fx
        .driver
        .prove(fx.input("CraftWood"), "retry", &NoopExecutionReporter)
        .unwrap_err();
    assert!(err.to_string().contains("awaiting the chain"), "{err}");

    fx.relayer.lock().unwrap().fail_submit = false;
    let submitted = fx.driver.submit(&tx_final_of(record)).unwrap();
    assert_eq!(submitted.state, TransactionState::Submitted);
    assert_eq!(submitted.relayer_job_id.as_deref(), Some("job-1"));
    fx.driver.submit(&tx_final_of(record)).unwrap();
    assert_eq!(fx.relayer.lock().unwrap().submitted.len(), 1);
}

#[test]
fn prove_holds_its_inputs_until_recorded() {
    let fx = fixture("log_1.dobj");
    let first = fx
        .driver
        .prove(fx.input("CraftWood"), "run-1", &NoopExecutionReporter)
        .unwrap();
    assert_eq!(
        fx.summary(&fx.input_file).held_by_run_id.as_deref(),
        Some("run-1")
    );
    assert!(
        !fx.driver
            .check_action(&craft_basics("CraftWood"))
            .unwrap()
            .feasible
    );
    let err = fx
        .driver
        .prove(fx.input("CraftWood"), "run-2", &NoopExecutionReporter)
        .unwrap_err();
    assert!(err.to_string().contains("held by run-1"), "{err}");
    assert!(fx.driver.list_transactions().unwrap().is_empty());
    assert_eq!(load_object_files(&fx.paths).unwrap().len(), 1);

    drop(first);
    assert_eq!(fx.summary(&fx.input_file).held_by_run_id, None);
    let second = fx
        .driver
        .prove(fx.input("CraftWood"), "run-2", &NoopExecutionReporter)
        .unwrap();
    let record = fx.driver.record(second).unwrap();
    assert_eq!(record.state, TransactionState::Recorded);
    assert_eq!(record.owner, "run-2");
    assert_eq!(record.input_files, vec![fx.input_file.clone()]);
    assert_eq!(
        fx.summary(&fx.input_file).held_by_run_id.as_deref(),
        Some("run-2")
    );
    for file_name in &record.output_files {
        let stored = fx.stored(file_name);
        assert_eq!(stored.status, FileStatus::Unknown);
        assert_eq!(stored.tx_final, tx_final_of(&record));
    }
    assert_eq!(
        crate::transactions::read_payload(&fx.paths, &tx_final_of(&record)).unwrap(),
        vec![1, 2, 3]
    );
}

#[test]
fn proving_reports_local_stage_boundaries() {
    #[derive(Clone, Default)]
    struct Reporter(Arc<Mutex<Vec<wire_types::ExecutionStage>>>);

    impl crate::ExecutionReporter for Reporter {
        fn on_step(
            &self,
            _: wire_types::ExecutionPhase,
            _: &str,
            ctx: &crate::ExecutionStepContext,
        ) {
            if let Some(stage) = ctx.stage {
                let mut stages = self.0.lock().unwrap();
                if stages.last() != Some(&stage) {
                    stages.push(stage);
                }
            }
        }
    }

    let fx = fixture("log_1.dobj");
    let reporter = Reporter::default();
    let proved = fx
        .driver
        .prove(fx.input("CraftWood"), "run-1", &reporter)
        .unwrap();
    use wire_types::ExecutionStage::*;
    assert_eq!(
        *reporter.0.lock().unwrap(),
        [
            VerifyingInputs,
            LoadingModule,
            Building,
            Mining,
            Building,
            Solving,
            Proving,
            Shrinking
        ]
    );
    assert_eq!(proved.input_files(), &[fx.input_file]);
}

#[test]
fn input_hold_notification_precedes_proving_and_unwinds_with_the_holds() {
    #[derive(Clone)]
    struct Reporter {
        driver: Arc<Driver>,
        notified: Arc<std::sync::atomic::AtomicBool>,
        panic: bool,
    }
    impl crate::ExecutionReporter for Reporter {
        fn on_inputs_held(&self, files: &[String]) {
            assert_eq!(files, ["log_1.dobj"]);
            assert_eq!(
                self.driver
                    .read_object(std::path::Path::new(&files[0]))
                    .unwrap()
                    .held_by_run_id
                    .as_deref(),
                Some("run-1")
            );
            self.notified
                .store(true, std::sync::atomic::Ordering::Relaxed);
            assert!(!self.panic, "reporter failed");
        }
        fn on_step(
            &self,
            _: wire_types::ExecutionPhase,
            message: &str,
            _: &crate::ExecutionStepContext,
        ) {
            if message == wire_types::ExecutionStage::Building.message() {
                assert!(self.notified.load(std::sync::atomic::Ordering::Relaxed));
            }
        }
    }
    for panic in [false, true] {
        let fx = fixture("log_1.dobj");
        let notified = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let input = fx.input("CraftWood");
        let driver = Arc::new(fx.driver);
        let reporter = Reporter {
            driver: driver.clone(),
            notified: notified.clone(),
            panic,
        };
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            driver.prove(input, "run-1", &reporter)
        }));
        assert!(notified.load(std::sync::atomic::Ordering::Relaxed));
        if panic {
            assert!(outcome.is_err());
        } else {
            drop(outcome.unwrap().unwrap());
        }
        assert_eq!(
            driver
                .read_object(std::path::Path::new(&fx.input_file))
                .unwrap()
                .held_by_run_id,
            None
        );
        assert!(driver.list_transactions().unwrap().is_empty());
    }
}

#[test]
fn record_refuses_an_output_file_that_exists() {
    let fx = fixture("log_1.dobj");
    let proved = fx
        .driver
        .prove(fx.input("CraftWood"), "run-1", &NoopExecutionReporter)
        .unwrap();
    let output_files = proved.output_files().unwrap();
    let squatter = ObjectRecord {
        content_hash: proved.outputs[0].obj.commitment(),
        class: proved.outputs[0].class.clone(),
        status: FileStatus::Unknown,
        tx_hash: None,
        obj: proved.outputs[0].obj.clone(),
        tx_final: Hash::default(),
    };
    write_object_file(&fx.paths, &squatter, &output_files[0]).unwrap();

    let err = fx.driver.record(proved).unwrap_err();
    assert!(
        err.to_string().contains("output file already exists"),
        "{err}"
    );
    assert_eq!(fx.summary(&fx.input_file).held_by_run_id, None);
    assert!(fx.driver.list_transactions().unwrap().is_empty());
    assert_eq!(
        std::fs::read_dir(&fx.paths.transactions_dir)
            .unwrap()
            .count(),
        0
    );
    assert_eq!(fx.stored(&output_files[0]).tx_final, Hash::default());
}

#[test]
fn delete_removes_a_recorded_transaction() {
    let fx = fixture("log_1.dobj");
    let proved = fx
        .driver
        .prove(fx.input("CraftWood"), "run-1", &NoopExecutionReporter)
        .unwrap();
    let record = fx.driver.record(proved).unwrap();
    let tx_final = tx_final_of(&record);

    let deleted = fx.driver.delete_transaction(&tx_final).unwrap();
    assert_eq!(deleted.record, record);
    assert_eq!(deleted.removed_files, record.output_files);
    assert!(fx.driver.list_transactions().unwrap().is_empty());
    assert!(!fx.paths.transactions_dir.join(&record.tx_final).exists());
    for file_name in &record.output_files {
        assert!(!fx.paths.objects_dir.join(file_name).exists());
    }
    let err = fx.driver.delete_transaction(&tx_final).unwrap_err();
    assert!(matches!(
        err.downcast_ref::<crate::DriverError>(),
        Some(crate::DriverError::TransactionNotFound(_))
    ));
    assert!(
        fx.driver
            .check_action(&craft_basics("CraftWood"))
            .unwrap()
            .feasible
    );
    fx.driver
        .prove(fx.input("CraftWood"), "run-2", &NoopExecutionReporter)
        .unwrap();
}

#[test]
fn delete_refuses_a_submitted_transaction() {
    let fx = fixture("log_1.dobj");
    let result = fx.driver.execute(fx.input("CraftWood")).unwrap();
    let err = fx
        .driver
        .delete_transaction(&tx_final_of(&result))
        .unwrap_err();
    assert!(err.to_string().contains("is submitted"), "{err}");
    assert_eq!(fx.driver.list_transactions().unwrap().len(), 1);
}

#[test]
fn settle_expires_an_aged_grounding_and_frees_its_inputs() {
    let fx = fixture("log_1.dobj");
    let result = fx.driver.execute(fx.input("CraftWood")).unwrap();
    let record = fx.driver.get_transaction(&tx_final_of(&result)).unwrap();

    fx.advance_head_to(record.grounding_block + MAX_STATE_ROOT_AGE_BLOCKS);
    let outcome = fx.driver.settle().unwrap();
    assert!(
        outcome.records.is_empty(),
        "at the limit the transaction can still land"
    );

    fx.advance_head_to(record.grounding_block + MAX_STATE_ROOT_AGE_BLOCKS + 1);
    let outcome = fx.driver.settle().unwrap();
    assert_eq!(outcome.records.len(), 1);
    let expired = &outcome.records[0];
    assert_eq!(expired.state, TransactionState::Expired);
    assert!(
        expired.reason.as_deref().unwrap().contains("behind"),
        "{expired:?}"
    );
    for file_name in &record.output_files {
        assert!(!fx.paths.objects_dir.join(file_name).exists());
    }
    assert_eq!(fx.stored(&fx.input_file).status, FileStatus::Live);
    assert_eq!(fx.summary(&fx.input_file).status, ObjectStatus::Live);
    assert!(
        fx.driver
            .check_action(&craft_basics("CraftWood"))
            .unwrap()
            .feasible
    );

    fx.land(&record);
    let outcome = fx.driver.settle().unwrap();
    assert!(outcome.records.is_empty());
    assert_eq!(
        fx.driver
            .get_transaction(&tx_final_of(&result))
            .unwrap()
            .state,
        TransactionState::Expired
    );

    let proved = fx
        .driver
        .prove(fx.input("CraftWood"), "again", &NoopExecutionReporter)
        .unwrap();
    fx.driver.record(proved).unwrap();
}

#[test]
fn submit_refuses_an_aged_grounding() {
    let fx = fixture("log_1.dobj");
    let proved = fx
        .driver
        .prove(fx.input("CraftWood"), "run-1", &NoopExecutionReporter)
        .unwrap();
    let record = fx.driver.record(proved).unwrap();
    fx.advance_head_to(record.grounding_block + MAX_STATE_ROOT_AGE_BLOCKS + 1);

    let err = fx.driver.submit(&tx_final_of(&record)).unwrap_err();
    assert!(
        err.to_string().contains("behind the synchronizer head"),
        "{err}"
    );
    assert!(fx.relayer.lock().unwrap().submitted.is_empty());
    let unchanged = fx.driver.get_transaction(&tx_final_of(&record)).unwrap();
    assert_eq!(unchanged, record);
    fx.driver.delete_transaction(&tx_final_of(&record)).unwrap();
}

#[test]
fn settle_annotates_a_relayer_failure_and_still_lands() {
    let fx = fixture("log_1.dobj");
    let result = fx.driver.execute(fx.input("CraftWood")).unwrap();
    {
        let mut relayer = fx.relayer.lock().unwrap();
        relayer.status = Some(JobStatus::Failed);
        relayer.tx_hash = Some("0xbroadcast".to_string());
        relayer.last_error = Some("nonce too low".to_string());
    }
    let outcome = fx.driver.settle().unwrap();
    assert_eq!(outcome.records.len(), 1);
    let annotated = &outcome.records[0];
    assert_eq!(annotated.state, TransactionState::Submitted);
    assert_eq!(annotated.relayer_failure.as_deref(), Some("nonce too low"));
    assert_eq!(annotated.tx_hash.as_deref(), Some("0xbroadcast"));
    for file_name in &result.output_files {
        let summary = fx.summary(file_name);
        assert_eq!(summary.status, ObjectStatus::Pending);
        assert_eq!(summary.tx_hash.as_deref(), Some("0xbroadcast"));
    }

    fx.land_with_tx_hash(annotated, "0xincluded");
    let outcome = fx.driver.settle().unwrap();
    assert_eq!(outcome.records[0].state, TransactionState::Landed);
    assert_eq!(
        outcome.records[0].relayer_failure.as_deref(),
        Some("nonce too low")
    );
    assert_eq!(outcome.records[0].tx_hash.as_deref(), Some("0xincluded"));
    for file_name in &result.output_files {
        assert_eq!(fx.summary(file_name).status, ObjectStatus::Live);
    }
}

#[test]
fn settle_with_an_unreachable_synchronizer_changes_nothing() {
    let fx = fixture("log_1.dobj");
    let result = fx.driver.execute(fx.input("CraftWood")).unwrap();
    fx.chain.lock().unwrap().fail_status = true;
    let err = fx.driver.settle().unwrap_err();
    assert!(err.to_string().contains("unreachable"), "{err}");
    assert_eq!(
        fx.driver
            .get_transaction(&tx_final_of(&result))
            .unwrap()
            .state,
        TransactionState::Submitted
    );
}

#[test]
fn transactions_without_a_record_file_are_not_transactions() {
    let fx = fixture("log_1.dobj");
    let orphan = Hash::default();
    crate::transactions::write_payload(&fx.paths, &orphan, b"half written").unwrap();
    assert!(fx.driver.list_transactions().unwrap().is_empty());
    assert!(matches!(
        fx.driver
            .get_transaction(&orphan)
            .unwrap_err()
            .downcast_ref::<crate::DriverError>(),
        Some(crate::DriverError::TransactionNotFound(_))
    ));
    // A crash before record persistence leaves an output without a transaction record.
    let stray = ObjectRecord {
        status: FileStatus::Unknown,
        tx_final: orphan,
        ..fx.stored(&fx.input_file)
    };
    write_object_file(&fx.paths, &stray, "stray.dobj").unwrap();
    assert_eq!(fx.summary("stray.dobj").status, ObjectStatus::Unknown);
    assert!(fx.driver.settle().unwrap().objects.is_empty());
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
    let fx = fixture("forged_wood.dobj");
    let forged = ObjectRecord {
        class: craft_basics("Wood"),
        ..fx.stored(&fx.input_file)
    };
    write_object_file(&fx.paths, &forged, &fx.input_file).unwrap();

    let err = fx.driver.execute(fx.input("CraftSticks")).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("class hash mismatch"),
        "expected hash-mismatch error, got: {msg}"
    );
    assert_eq!(fx.summary(&fx.input_file).held_by_run_id, None);
}

/// Serialized Log, producing transaction hash, and nullifier for import tests.
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
    let commitment = spendable.obj.commitment();
    let nullifier = txlib::object_nullifier_hash(&spendable.obj).unwrap();
    let record = ObjectRecord {
        content_hash: commitment,
        class: craft_basics("Log"),
        status: FileStatus::Live,
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
) -> (Driver, Arc<Mutex<MockChain>>) {
    let paths = temp_paths();
    ensure_store_dirs(&paths).unwrap();
    let (deps, chain) = mock_deps();
    {
        let mut chain = chain.lock().unwrap();
        chain.created = grounded;
        chain.nullifiers = nullifiers;
        chain.fail_membership = fail_membership;
    }
    (Driver::open(paths, deps).unwrap(), chain)
}

#[test]
fn test_import_grounded_object_is_live() {
    let (json, source_tx, _nullifier) = make_importable_log();
    let (driver, _) = import_driver(HashSet::from([source_tx]), HashSet::new(), false);
    let summary = driver.import_object(&json, None).unwrap();
    assert_eq!(summary.class, craft_basics("Log"));
    assert_eq!(summary.status, ObjectStatus::Live);
    assert!(summary.file_name.starts_with("craft-basics__log_"));
}

#[test]
fn imported_objects_share_their_key_until_an_action_replaces_it() {
    use pod2::middleware::{StrKey, Value};

    let (json, source_tx, _nullifier) = make_importable_log();
    let (driver, _) = import_driver(HashSet::from([source_tx]), HashSet::new(), false);
    assert!(driver.import_object(&json, None).unwrap().shared_key);

    let imported: ObjectRecord = serde_json::from_str(&json).unwrap();
    let file_version = |obj: pod2::middleware::containers::Dictionary| {
        let record = ObjectRecord {
            content_hash: obj.commitment(),
            obj,
            ..imported.clone()
        };
        let file_name = object_file_name(&record.class, &record.obj).unwrap();
        write_object_file(driver.paths(), &record, &file_name).unwrap();
        driver
            .read_object(std::path::Path::new(&file_name))
            .unwrap()
    };

    let mut changed_keeping_key = imported.obj.clone();
    changed_keeping_key
        .insert(&StrKey::from("durability"), &Value::from(1))
        .unwrap();
    assert!(file_version(changed_keeping_key).shared_key);

    let mut rekeyed = imported.obj.clone();
    rekeyed
        .update(&StrKey::from("key"), &Value::from(42))
        .unwrap();
    assert!(!file_version(rekeyed).shared_key);
}

#[test]
fn test_import_ungrounded_object_is_unknown() {
    let (json, _source_tx, _nullifier) = make_importable_log();
    let (driver, _) = import_driver(HashSet::new(), HashSet::new(), false);
    let summary = driver.import_object(&json, None).unwrap();
    assert_eq!(summary.status, ObjectStatus::Unknown);
}

#[test]
fn sync_objects_follows_the_chain_forward_only() {
    let (json, commitment, nullifier) = make_importable_log();
    let (driver, chain) = import_driver(HashSet::new(), HashSet::new(), false);
    let summary = driver.import_object(&json, None).unwrap();
    assert_eq!(summary.status, ObjectStatus::Unknown);

    chain.lock().unwrap().created.insert(commitment);
    let synced = driver.sync_objects(None).unwrap().objects;
    assert_eq!(synced.len(), 1);
    assert_eq!(synced[0].status, ObjectStatus::Live);

    chain.lock().unwrap().nullifiers.insert(nullifier);
    let synced = driver.sync_objects(None).unwrap().objects;
    assert_eq!(synced[0].status, ObjectStatus::Nullified);
    assert!(
        driver
            .paths()
            .nullified_objects_dir
            .join(&summary.file_name)
            .exists()
    );

    chain.lock().unwrap().nullifiers.clear();
    let synced = driver.sync_objects(None).unwrap().objects;
    assert_eq!(synced[0].status, ObjectStatus::Nullified);

    let query = ObjectQuery {
        exclude_nullified: true,
        ..ObjectQuery::default()
    };
    assert!(
        driver
            .sync_objects(Some(&query))
            .unwrap()
            .objects
            .is_empty()
    );
}

#[test]
fn test_read_object_raw_returns_stored_file() {
    let (json, source_tx, _nullifier) = make_importable_log();
    let (driver, _) = import_driver(HashSet::from([source_tx]), HashSet::new(), false);
    let summary = driver.import_object(&json, None).unwrap();
    let raw = driver
        .read_object_raw(std::path::Path::new(&summary.file_name))
        .unwrap();
    let on_disk =
        std::fs::read_to_string(driver.paths().objects_dir.join(&summary.file_name)).unwrap();
    assert_eq!(raw, on_disk);
}

#[test]
fn test_read_object_raw_missing_file_is_not_found() {
    let (driver, _) = import_driver(HashSet::new(), HashSet::new(), false);
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
    let (driver, _) = import_driver(HashSet::new(), HashSet::new(), false);
    let record: ObjectRecord = serde_json::from_str(&json).unwrap();
    let record = ObjectRecord {
        status: FileStatus::Unknown,
        ..record
    };
    write_object_file(driver.paths(), &record, "stale.dobj").unwrap();
    let on_disk = std::fs::read_to_string(driver.paths().objects_dir.join("stale.dobj")).unwrap();

    let summary = driver
        .discard_object_unsafe(std::path::Path::new("stale.dobj"))
        .unwrap();
    assert_eq!(summary.status, ObjectStatus::Unknown);
    assert!(driver.list_objects(None).unwrap().is_empty());
    let discarded =
        std::fs::read_to_string(driver.paths().discarded_objects_dir.join("stale.dobj")).unwrap();
    assert_eq!(discarded, on_disk);

    let spent = ObjectRecord {
        status: FileStatus::Nullified,
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
    let (driver, _) = import_driver(HashSet::new(), HashSet::new(), false);
    let live_record: ObjectRecord = serde_json::from_str(&json).unwrap();
    let nullified_record = ObjectRecord {
        status: FileStatus::Nullified,
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
    let (mut deps, _) = mock_deps();
    deps.catalog = Arc::new(RwLock::new(Arc::new(empty)));
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
    let (driver, _) = import_driver(
        HashSet::from([source_tx]),
        HashSet::from([nullifier]),
        false,
    );
    let err = driver.import_object(&json, None).unwrap_err();
    assert!(
        err.to_string().contains("spent"),
        "expected already-spent error, got: {err}"
    );
}

#[test]
fn test_import_duplicate_is_rejected() {
    let (json, source_tx, _nullifier) = make_importable_log();
    let (driver, _) = import_driver(HashSet::from([source_tx]), HashSet::new(), false);
    driver.import_object(&json, None).unwrap();
    let err = driver.import_object(&json, None).unwrap_err();
    assert!(
        err.to_string().contains("already present"),
        "expected duplicate error, got: {err}"
    );
}

#[test]
fn imported_objects_keep_their_requested_file_name() {
    let (first, source_tx, _nullifier) = make_importable_log();
    let (second, _, _) = make_importable_log();
    let (driver, _) = import_driver(HashSet::from([source_tx]), HashSet::new(), false);

    let summary = driver.import_object(&first, Some("my log")).unwrap();
    assert_eq!(summary.file_name, "my log.dobj");
    assert_eq!(summary.status, ObjectStatus::Live);
    assert!(driver.paths().objects_dir.join("my log.dobj").exists());
    let summary = driver.import_object(&second, Some("my log.dobj")).unwrap();
    assert_eq!(summary.file_name, "my log 2.dobj");

    let err = driver
        .import_object(&first, Some("another name.dobj"))
        .unwrap_err();
    assert!(
        err.to_string().contains("already present: my log.dobj"),
        "expected duplicate error naming the held file, got: {err}"
    );
}

#[test]
fn imported_file_names_skip_discarded_files_and_refuse_foreign_generated_names() {
    let (json, _source_tx, _nullifier) = make_importable_log();
    let (driver, _) = import_driver(HashSet::new(), HashSet::new(), false);
    std::fs::create_dir_all(&driver.paths().discarded_objects_dir).unwrap();
    std::fs::write(driver.paths().discarded_objects_dir.join("log.dobj"), "{}").unwrap();

    let err = driver
        .import_object(
            &json,
            Some("craft-basics__log_0123456789abcdef_0123456789abcdef.dobj"),
        )
        .unwrap_err();
    assert!(
        err.to_string().contains("generated object name"),
        "expected generated-name error, got: {err}"
    );
    let err = driver
        .import_object(&json, Some("../log.dobj"))
        .unwrap_err();
    assert!(err.to_string().contains("invalid input"), "got: {err}");

    let summary = driver.import_object(&json, Some("log.dobj")).unwrap();
    assert_eq!(summary.file_name, "log 2.dobj");
}

#[test]
fn imported_objects_may_keep_their_own_generated_name() {
    let (json, _source_tx, _nullifier) = make_importable_log();
    let record: ObjectRecord = serde_json::from_str(&json).unwrap();
    let generated = object_file_name(&record.class, &record.obj).unwrap();
    let (driver, _) = import_driver(HashSet::new(), HashSet::new(), false);
    let summary = driver.import_object(&json, Some(&generated)).unwrap();
    assert_eq!(summary.file_name, generated);
}

#[test]
fn test_import_sync_unreachable_falls_back_to_unknown() {
    let (json, _source_tx, _nullifier) = make_importable_log();
    let (driver, _) = import_driver(HashSet::new(), HashSet::new(), true);
    let summary = driver.import_object(&json, None).unwrap();
    assert_eq!(summary.status, ObjectStatus::Unknown);
}

#[test]
fn test_import_unknown_class_is_rejected() {
    let (json, _source_tx, _nullifier) = make_importable_log();
    let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
    value["class"]["name"] = serde_json::Value::String("Diamond".to_string());
    let tampered = serde_json::to_string(&value).unwrap();
    let (driver, _) = import_driver(HashSet::new(), HashSet::new(), false);
    let err = driver.import_object(&tampered, None).unwrap_err();
    assert!(
        err.to_string().contains("unknown class"),
        "expected unknown-class error, got: {err}"
    );
}

#[test]
fn test_import_content_hash_mismatch_is_rejected() {
    let (json, commitment, _nullifier) = make_importable_log();
    let (driver, _) = import_driver(HashSet::from([commitment]), HashSet::new(), false);
    let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
    value["contentHash"] = serde_json::Value::String("00".repeat(32));
    let tampered = serde_json::to_string(&value).unwrap();
    let err = driver.import_object(&tampered, None).unwrap_err();
    assert!(
        err.to_string().contains("content hash mismatch"),
        "expected content-hash error, got: {err}"
    );
    assert!(load_object_files(driver.paths()).unwrap().is_empty());

    let summary = driver.import_object(&json, None).unwrap();
    assert_eq!(summary.content_hash, encode_hash_hex(&commitment));
}

#[test]
fn test_import_class_hash_mismatch_is_rejected() {
    // Forge a real Log's class to craft-basics::Wood: the qualified name is a
    // known class, but the pod's `type` hash is IsLog, so the cryptographic
    // hash check must fire -- the same guarantee execute relies on.
    let (json, _source_tx, _nullifier) = make_importable_log();
    let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
    value["class"]["name"] = serde_json::Value::String("Wood".to_string());
    let tampered = serde_json::to_string(&value).unwrap();
    let (driver, _) = import_driver(HashSet::new(), HashSet::new(), false);
    let err = driver.import_object(&tampered, None).unwrap_err();
    assert!(
        err.to_string().contains("class hash mismatch"),
        "expected hash-mismatch error, got: {err}"
    );
}

#[test]
fn settle_skips_records_committed_during_status_query() {
    let fx = fixture("log_1.dobj");
    let anchor = fx.driver.execute(fx.input("CraftWood")).unwrap();
    let late = fx
        .driver
        .prove(
            ExecuteActionInput {
                action: craft_basics("FindLog"),
                input_objects: vec![],
                args: Default::default(),
            },
            "late-run",
            &NoopExecutionReporter,
        )
        .unwrap();
    let late_tx_final = late.tx_final;
    let late_outputs = late.output_files().unwrap();
    let driver = fx.driver.clone();
    fx.chain.lock().unwrap().on_status = Some(Box::new(move || {
        driver.record(late).unwrap();
    }));
    fx.advance_head_to(anchor.grounding_block + MAX_STATE_ROOT_AGE_BLOCKS + 1);

    let outcome = fx.driver.settle().unwrap();
    assert_eq!(outcome.records.len(), 1);
    assert_eq!(outcome.records[0].tx_final, anchor.tx_final);
    assert_eq!(outcome.open_records, 1);
    assert_eq!(
        fx.driver.get_transaction(&late_tx_final).unwrap().state,
        TransactionState::Recorded
    );
    for file in &late_outputs {
        assert!(fx.paths.objects_dir.join(file).is_file());
    }

    let outcome = fx.driver.settle().unwrap();
    assert_eq!(outcome.records.len(), 1);
    assert_eq!(outcome.records[0].state, TransactionState::Expired);
    for file in &late_outputs {
        assert!(!fx.paths.objects_dir.join(file).exists());
    }
}

#[test]
fn record_rolls_back_outputs_and_payload_when_persistence_fails() {
    for blocked_path in [
        crate::paths::PAYLOAD_FILE.to_string(),
        format!("{}.tmp", crate::paths::RECORD_FILE),
    ] {
        let fx = fixture("log_1.dobj");
        let proved = fx
            .driver
            .prove(fx.input("CraftWood"), "run-1", &NoopExecutionReporter)
            .unwrap();
        let output_files = proved.output_files().unwrap();
        assert!(!output_files.is_empty());
        let transaction_dir = crate::transactions::transaction_dir(&fx.paths, &proved.tx_final);
        std::fs::create_dir_all(transaction_dir.join(blocked_path)).unwrap();

        fx.driver.record(proved).unwrap_err();
        for file in output_files {
            assert!(!fx.paths.objects_dir.join(file).exists());
        }
        assert!(!transaction_dir.exists());
        assert!(fx.driver.list_transactions().unwrap().is_empty());
        assert_eq!(fx.summary(&fx.input_file).held_by_run_id, None);
        assert_eq!(fx.stored(&fx.input_file).status, FileStatus::Live);
        fx.driver
            .prove(fx.input("CraftWood"), "run-2", &NoopExecutionReporter)
            .unwrap();
    }
}

#[test]
fn landed_objects_keep_creation_hashes_after_transaction_pruning_and_restart() {
    let fx = generated_fixture();
    let mut input = fx.stored(&fx.input_file);
    input.tx_hash = Some("original-creation".to_string());
    write_object_file(&fx.paths, &input, &fx.input_file).unwrap();
    let record = fx.driver.execute(fx.input("CraftWood")).unwrap();
    fx.land_with_tx_hash(&record, "landed-creation");
    let landed = fx.driver.settle().unwrap().records.remove(0);
    let cleanup_at = landed.settled_at_ms.unwrap() + crate::TRANSACTION_RETENTION_MS;
    let retained = fx.driver.prune_transactions(cleanup_at - 1).unwrap();
    assert!(retained.tx_finals.is_empty());
    assert_eq!(retained.next_cleanup_at_ms, Some(cleanup_at));
    assert!(fx.paths.transactions_dir.join(&landed.tx_final).is_dir());

    let pruned = fx.driver.prune_transactions(cleanup_at).unwrap();
    assert_eq!(pruned.tx_finals, vec![landed.tx_final.clone()]);
    assert_eq!(pruned.next_cleanup_at_ms, None);
    assert!(!fx.paths.transactions_dir.join(&landed.tx_final).exists());
    assert_eq!(
        fx.stored(&fx.input_file).tx_hash.as_deref(),
        Some("original-creation")
    );
    let restarted = Driver::open(fx.paths.clone(), fx.driver.deps.clone()).unwrap();
    assert!(restarted.list_transactions().unwrap().is_empty());
    for file in &record.output_files {
        let stored = fx.stored(file);
        assert_eq!(stored.status, FileStatus::Live);
        assert_eq!(stored.tx_hash.as_deref(), Some("landed-creation"));
        let summary = restarted.read_object(std::path::Path::new(file)).unwrap();
        assert_eq!(summary.tx_hash, stored.tx_hash);
    }
}

#[test]
fn pruning_never_removes_open_transactions_or_their_reservations() {
    for submit in [false, true] {
        let fx = fixture("log_1.dobj");
        let proved = fx
            .driver
            .prove(fx.input("CraftWood"), "run", &NoopExecutionReporter)
            .unwrap();
        let record = fx.driver.record(proved).unwrap();
        if submit {
            fx.driver.submit(&tx_final_of(&record)).unwrap();
        }
        let outcome = fx
            .driver
            .prune_transactions(record.recorded_at_ms + crate::TRANSACTION_RETENTION_MS * 10)
            .unwrap();
        assert!(outcome.tx_finals.is_empty());
        assert!(crate::transactions::read_payload(&fx.paths, &tx_final_of(&record)).is_ok());
        assert!(
            fx.driver
                .get_transaction(&tx_final_of(&record))
                .unwrap()
                .is_open()
        );
        assert_eq!(
            fx.summary(&fx.input_file).held_by_run_id.as_deref(),
            Some("run")
        );
    }
}

#[test]
fn expired_transactions_are_pruned_after_outputs_are_removed() {
    let fx = fixture("log_1.dobj");
    let record = fx.driver.execute(fx.input("CraftWood")).unwrap();
    fx.advance_head_to(record.grounding_block + MAX_STATE_ROOT_AGE_BLOCKS + 1);
    let expired = fx.driver.settle().unwrap().records.remove(0);
    let cleanup_at = expired.settled_at_ms.unwrap() + crate::TRANSACTION_RETENTION_MS;
    assert!(
        fx.driver
            .prune_transactions(cleanup_at - 1)
            .unwrap()
            .tx_finals
            .is_empty()
    );
    assert_eq!(
        fx.driver.prune_transactions(cleanup_at).unwrap().tx_finals,
        vec![expired.tx_final]
    );
    assert_eq!(fx.summary(&fx.input_file).held_by_run_id, None);
    assert_eq!(fx.stored(&fx.input_file).status, FileStatus::Live);
    for file in &record.output_files {
        assert!(!fx.paths.objects_dir.join(file).exists());
    }
}

#[test]
fn pruning_repairs_committed_settlement_after_restart_and_retains_unreadable_outputs() {
    let fx = generated_fixture();
    let mut record = fx.driver.execute(fx.input("CraftWood")).unwrap();
    record.state = TransactionState::Landed;
    record.settled_at_ms = Some(1);
    record.tx_hash = Some("creation".to_string());
    crate::transactions::write_record(&fx.paths, &record).unwrap();
    let file = &record.output_files[0];
    let output = fx.stored(file);
    std::fs::write(fx.paths.objects_dir.join(file), "broken JSON").unwrap();
    let restarted = Driver::open(fx.paths.clone(), fx.driver.deps.clone()).unwrap();
    let cleanup_at = 1 + crate::TRANSACTION_RETENTION_MS;
    let retained = restarted.prune_transactions(cleanup_at).unwrap();
    assert!(retained.tx_finals.is_empty());
    assert!(retained.next_cleanup_at_ms.is_some());
    assert!(restarted.get_transaction(&tx_final_of(&record)).is_ok());
    write_object_file(&fx.paths, &output, file).unwrap();
    assert_eq!(
        restarted.prune_transactions(cleanup_at).unwrap().tx_finals,
        vec![record.tx_final]
    );
    assert_eq!(fx.stored(&fx.input_file).status, FileStatus::Nullified);
    for file in &record.output_files {
        assert_eq!(fx.stored(file).status, FileStatus::Live);
        assert_eq!(fx.stored(file).tx_hash.as_deref(), Some("creation"));
    }
}

#[test]
fn completed_reconciliation_is_cached_until_pruning_is_due() {
    let fx = fixture("log_1.dobj");
    let record = fx.driver.execute(fx.input("CraftWood")).unwrap();
    fx.land(&record);
    let landed = fx.driver.settle().unwrap().records.remove(0);
    let settled_at = landed.settled_at_ms.unwrap();
    let cleanup_at = settled_at + crate::TRANSACTION_RETENTION_MS;
    fx.driver.prune_transactions(settled_at).unwrap();
    let file = &record.output_files[0];
    let output = fx.stored(file);
    std::fs::write(fx.paths.objects_dir.join(file), "broken JSON").unwrap();
    let retained = fx
        .driver
        .clone()
        .prune_transactions(settled_at + 1)
        .unwrap();
    assert!(retained.tx_finals.is_empty());
    assert_eq!(retained.next_cleanup_at_ms, Some(cleanup_at));
    let blocked = fx.driver.prune_transactions(cleanup_at).unwrap();
    assert!(blocked.tx_finals.is_empty());
    assert_eq!(blocked.next_cleanup_at_ms, Some(cleanup_at + 60_000));
    assert!(fx.driver.get_transaction(&tx_final_of(&record)).is_ok());
    write_object_file(&fx.paths, &output, file).unwrap();
    assert_eq!(
        fx.driver
            .prune_transactions(cleanup_at + 1)
            .unwrap()
            .tx_finals,
        vec![record.tx_final]
    );
}

#[test]
fn reconciliation_cache_rechecks_changed_records_and_after_restart() {
    let fx = fixture("log_1.dobj");
    let record = fx.driver.execute(fx.input("CraftWood")).unwrap();
    fx.land(&record);
    let mut landed = fx.driver.settle().unwrap().records.remove(0);
    let settled_at = landed.settled_at_ms.unwrap();
    fx.driver.prune_transactions(settled_at).unwrap();
    landed.tx_hash = Some("late-creation".to_string());
    crate::transactions::write_record(&fx.paths, &landed).unwrap();
    fx.driver.prune_transactions(settled_at + 1).unwrap();
    for file in &record.output_files {
        assert_eq!(fx.stored(file).tx_hash.as_deref(), Some("late-creation"));
    }
    let file = &record.output_files[0];
    let mut output = fx.stored(file);
    output.tx_hash = None;
    output.status = FileStatus::Unknown;
    write_object_file(&fx.paths, &output, file).unwrap();
    let restarted = Driver::open(fx.paths.clone(), fx.driver.deps.clone()).unwrap();
    let recovered = restarted.prune_transactions(settled_at + 2).unwrap();
    assert!(recovered.tx_finals.is_empty());
    assert!(
        recovered
            .objects
            .iter()
            .any(|object| object.file_name == *file
                && object.tx_hash.as_deref() == Some("late-creation"))
    );
    assert_eq!(fx.stored(file).status, FileStatus::Live);
    assert_eq!(fx.stored(file).tx_hash.as_deref(), Some("late-creation"));
}

#[test]
fn object_hash_metadata_is_optional_and_survives_import() {
    let (json, commitment, _) = make_importable_log();
    let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(value.get("txHash").is_none());
    let legacy: ObjectRecord = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(legacy.tx_hash, None);
    value["txHash"] = serde_json::json!("creation");
    let (driver, _) = import_driver(HashSet::from([commitment]), HashSet::new(), false);
    let summary = driver.import_object(&value.to_string(), None).unwrap();
    assert_eq!(summary.tx_hash.as_deref(), Some("creation"));
    assert_eq!(
        driver
            .read_object(std::path::Path::new(&summary.file_name))
            .unwrap()
            .tx_hash,
        summary.tx_hash
    );
}

#[test]
fn unattempted_records_use_only_head_checks_and_expire_safely() {
    let fx = fixture("log_1.dobj");
    let proved = fx
        .driver
        .prove(fx.input("CraftWood"), "staged", &NoopExecutionReporter)
        .unwrap();
    let record = fx.driver.record(proved).unwrap();
    fx.chain.lock().unwrap().fail_status = true;
    fx.advance_head_to(record.grounding_block + MAX_STATE_ROOT_AGE_BLOCKS);
    let outcome = fx.driver.settle().unwrap();
    assert!(outcome.records.is_empty());
    assert_eq!(outcome.open_records, 1);
    assert_eq!(outcome.unattempted_records, 1);
    assert_eq!(fx.chain.lock().unwrap().head_requests, 1);
    assert!(fx.chain.lock().unwrap().queried_statuses.is_empty());
    assert_eq!(
        fx.summary(&fx.input_file).held_by_run_id.as_deref(),
        Some("staged")
    );
    fx.advance_head_to(record.grounding_block + MAX_STATE_ROOT_AGE_BLOCKS + 1);
    fx.chain.lock().unwrap().fail_head = true;
    assert!(fx.driver.settle().is_err());
    assert_eq!(
        fx.driver
            .get_transaction(&tx_final_of(&record))
            .unwrap()
            .state,
        TransactionState::Recorded
    );
    fx.chain.lock().unwrap().fail_head = false;
    let outcome = fx.driver.settle().unwrap();
    assert_eq!(outcome.records[0].state, TransactionState::Expired);
    assert_eq!(outcome.unattempted_records, 0);
    assert!(fx.chain.lock().unwrap().queried_statuses.is_empty());
    assert_eq!(fx.summary(&fx.input_file).held_by_run_id, None);
    for file in &record.output_files {
        assert!(!fx.paths.objects_dir.join(file).exists());
    }
}

#[test]
fn attempted_records_without_acknowledgement_still_query_status() {
    let fx = fixture("log_1.dobj");
    fx.relayer.lock().unwrap().fail_submit = true;
    assert!(fx.driver.execute(fx.input("CraftWood")).is_err());
    let record = fx.driver.list_transactions().unwrap().remove(0);
    assert_eq!(record.relayer_job_id, None);
    fx.land_with_tx_hash(&record, "0xunacknowledged");
    fx.advance_head_to(record.grounding_block + MAX_STATE_ROOT_AGE_BLOCKS + 1);
    let outcome = fx.driver.settle().unwrap();
    assert_eq!(outcome.records[0].state, TransactionState::Landed);
    assert_eq!(
        outcome.records[0].tx_hash.as_deref(),
        Some("0xunacknowledged")
    );
    assert_eq!(fx.chain.lock().unwrap().queried_statuses.len(), 1);
    assert_eq!(outcome.unattempted_records, 0);
    for file in &record.output_files {
        assert_eq!(fx.stored(file).tx_hash.as_deref(), Some("0xunacknowledged"));
    }
}

#[test]
fn settle_expires_a_rejected_transaction_before_its_grounding_ages_out() {
    let fx = fixture("log_1.dobj");
    let record = fx.driver.execute(fx.input("CraftWood")).unwrap();
    fx.reject(&record, "created object already exists");
    let outcome = fx.driver.settle().unwrap();
    assert_eq!(outcome.open_records, 0);
    let expired = &outcome.records[0];
    assert_eq!(expired.state, TransactionState::Expired);
    assert_eq!(expired.tx_hash.as_deref(), Some("0xrejected"));
    let reason = expired.reason.as_deref().unwrap();
    assert!(
        reason.contains("rejected by the synchronizer")
            && reason.contains("created object already exists"),
        "{reason}"
    );
    assert_eq!(
        outcome.removed_files.len(),
        record.output_files.len(),
        "{outcome:?}"
    );
    for file in &record.output_files {
        assert!(!fx.paths.objects_dir.join(file).exists());
    }
    assert_eq!(fx.stored(&fx.input_file).status, FileStatus::Live);
    assert_eq!(fx.summary(&fx.input_file).held_by_run_id, None);
    assert!(
        fx.driver
            .check_action(&craft_basics("CraftWood"))
            .unwrap()
            .feasible
    );
}

#[test]
fn mixed_settlement_queries_only_attempted_transaction_statuses() {
    let fx = fixture("log_1.dobj");
    let submitted = fx.driver.execute(fx.input("CraftWood")).unwrap();
    let proved = fx
        .driver
        .prove(
            ExecuteActionInput {
                action: craft_basics("FindLog"),
                input_objects: vec![],
                args: Default::default(),
            },
            "staged",
            &NoopExecutionReporter,
        )
        .unwrap();
    let staged = fx.driver.record(proved).unwrap();
    assert_ne!(staged.tx_final, submitted.tx_final);
    fx.chain.lock().unwrap().head_requests = 0;
    let outcome = fx.driver.settle().unwrap();
    assert_eq!(outcome.open_records, 2);
    assert_eq!(outcome.unattempted_records, 1);
    let chain = fx.chain.lock().unwrap();
    assert_eq!(chain.head_requests, 1);
    assert_eq!(chain.queried_statuses, vec![tx_final_of(&submitted)]);
}

#[test]
fn submission_started_during_head_query_waits_for_a_status_before_expiry() {
    let fx = fixture("log_1.dobj");
    let proved = fx
        .driver
        .prove(fx.input("CraftWood"), "staged", &NoopExecutionReporter)
        .unwrap();
    let record = fx.driver.record(proved).unwrap();
    fx.advance_head_to(record.grounding_block + MAX_STATE_ROOT_AGE_BLOCKS + 1);
    let paths = fx.paths.clone();
    let mut submitted = record.clone();
    submitted.state = TransactionState::Submitted;
    submitted.submit_attempted_at_ms = Some(1);
    submitted.relayer_job_id = Some("job-1".to_string());
    fx.chain.lock().unwrap().on_head = Some(Box::new(move || {
        crate::transactions::write_record(&paths, &submitted).unwrap();
    }));
    fx.land(&record);
    let outcome = fx.driver.settle().unwrap();
    assert!(outcome.records.is_empty());
    assert_eq!(outcome.open_records, 1);
    assert_eq!(outcome.unattempted_records, 0);
    assert!(fx.chain.lock().unwrap().queried_statuses.is_empty());
    for file in &record.output_files {
        assert!(fx.paths.objects_dir.join(file).exists());
    }
    assert_eq!(
        fx.driver.settle().unwrap().records[0].state,
        TransactionState::Landed
    );
}

#[test]
fn concurrent_submits_of_one_transaction_share_the_accepted_job() {
    let fx = fixture("log_1.dobj");
    let proved = fx
        .driver
        .prove(fx.input("CraftWood"), "run", &NoopExecutionReporter)
        .unwrap();
    let record = fx.driver.record(proved).unwrap();
    let tx_final = tx_final_of(&record);
    let gate = fx.pause_submission(false);
    let first = {
        let driver = fx.driver.clone();
        std::thread::spawn(move || driver.submit(&tx_final))
    };
    gate.wait();
    let (started, ready) = std::sync::mpsc::channel();
    let (finished, completion) = std::sync::mpsc::channel();
    let second = {
        let driver = fx.driver.clone();
        std::thread::spawn(move || {
            started.send(()).unwrap();
            let result = driver.submit(&tx_final);
            finished.send(()).unwrap();
            result
        })
    };
    ready
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    let premature = completion.recv_timeout(std::time::Duration::from_millis(100));
    assert_eq!(
        fx.driver.get_transaction(&tx_final).unwrap().state,
        TransactionState::Recorded
    );
    assert!(!fx.driver.list_objects(None).unwrap().is_empty());
    drop(gate);
    let accepted = first.join().unwrap().unwrap();
    let repeated = second.join().unwrap().unwrap();
    assert!(
        premature.is_err(),
        "the overlapping call returned before acceptance"
    );
    assert_eq!(accepted, repeated);
    assert_eq!(accepted.state, TransactionState::Submitted);
    assert_eq!(fx.relayer.lock().unwrap().submit_calls, 1);
    assert_eq!(fx.chain.lock().unwrap().head_requests, 1);
}

#[test]
fn independent_transactions_can_submit_while_another_request_is_blocked() {
    let fx = fixture("log_1.dobj");
    let proved = fx
        .driver
        .prove(fx.input("CraftWood"), "first", &NoopExecutionReporter)
        .unwrap();
    let first_record = fx.driver.record(proved).unwrap();
    let proved = fx
        .driver
        .prove(
            ExecuteActionInput {
                action: craft_basics("FindLog"),
                input_objects: vec![],
                args: Default::default(),
            },
            "second",
            &NoopExecutionReporter,
        )
        .unwrap();
    let second_record = fx.driver.record(proved).unwrap();
    let gate = fx.pause_submission(false);
    let first = {
        let driver = fx.driver.clone();
        std::thread::spawn(move || driver.submit(&tx_final_of(&first_record)))
    };
    gate.wait();
    let (finished, completion) = std::sync::mpsc::channel();
    let second = {
        let driver = fx.driver.clone();
        std::thread::spawn(move || {
            let result = driver.submit(&tx_final_of(&second_record));
            finished.send(()).unwrap();
            result
        })
    };
    let completed_independently = completion.recv_timeout(std::time::Duration::from_secs(2));
    drop(gate);
    let first = first.join().unwrap().unwrap();
    let second = second.join().unwrap().unwrap();
    assert!(
        completed_independently.is_ok(),
        "an unrelated submission was blocked"
    );
    assert_ne!(first.relayer_job_id, second.relayer_job_id);
    assert_eq!(fx.relayer.lock().unwrap().submit_calls, 2);

    let next_lock = fx.driver.submission_lock(&Hash::default());
    let locks = fx.driver.submission_locks.lock().unwrap();
    assert_eq!(locks.len(), 1);
    assert!(locks.values().all(|lock| lock.strong_count() > 0));
    drop(locks);
    drop(next_lock);
}

#[test]
fn a_failed_submission_releases_the_lock_for_a_waiting_retry() {
    let fx = fixture("log_1.dobj");
    let proved = fx
        .driver
        .prove(fx.input("CraftWood"), "run", &NoopExecutionReporter)
        .unwrap();
    let record = fx.driver.record(proved).unwrap();
    let tx_final = tx_final_of(&record);
    let gate = fx.pause_submission(true);
    let first = {
        let driver = fx.driver.clone();
        std::thread::spawn(move || driver.submit(&tx_final))
    };
    gate.wait();
    let (started, ready) = std::sync::mpsc::channel();
    let second = {
        let driver = fx.driver.clone();
        std::thread::spawn(move || {
            started.send(()).unwrap();
            driver.submit(&tx_final)
        })
    };
    ready
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    drop(gate);
    let error = first.join().unwrap().unwrap_err();
    assert!(error.to_string().contains("blocked submission failed"));
    let retried = second.join().unwrap().unwrap();
    assert_eq!(retried.state, TransactionState::Submitted);
    assert!(retried.submit_attempted_at_ms.is_some());
    assert_eq!(fx.relayer.lock().unwrap().submit_calls, 2);
    assert_eq!(fx.relayer.lock().unwrap().submitted.len(), 1);
}

fn pending_name(file_name: &str, final_name: &str) -> wire_types::transactions::PendingName {
    wire_types::transactions::PendingName {
        file_name: file_name.to_string(),
        final_name: final_name.to_string(),
    }
}

#[test]
fn a_mutated_custom_named_object_takes_its_name_back_once_it_lands() {
    let fx = pick_fixture("my pick.dobj");
    let old_pick = fx.stored("my pick.dobj");
    let record = fx
        .driver
        .execute(fx.input("MineStoneWithWoodPick"))
        .unwrap();
    assert_eq!(record.input_files, vec!["my pick.dobj"]);
    assert_eq!(record.output_files[0], "my pick-pending.dobj");
    assert_eq!(
        record.pending_names,
        vec![pending_name("my pick-pending.dobj", "my pick.dobj")]
    );
    assert_eq!(
        fx.stored("my pick.dobj").content_hash,
        old_pick.content_hash
    );
    let new_pick = fx.stored("my pick-pending.dobj");
    assert_eq!(new_pick.status, FileStatus::Unknown);

    fx.land(&record);
    let outcome = fx.driver.settle().unwrap();
    assert!(!fx.paths.objects_dir.join("my pick-pending.dobj").exists());
    let renamed = fx.stored("my pick.dobj");
    assert_eq!(renamed.content_hash, new_pick.content_hash);
    assert_eq!(renamed.status, FileStatus::Live);
    let spent_name = object_file_name(&old_pick.class, &old_pick.obj).unwrap();
    let spent = crate::object_record::parse_object_record_file(
        &fx.paths.nullified_objects_dir.join(&spent_name),
    )
    .unwrap();
    assert_eq!(spent.content_hash, old_pick.content_hash);
    assert_eq!(spent.status, FileStatus::Nullified);
    assert!(
        outcome
            .removed_files
            .contains(&"my pick-pending.dobj".to_string())
    );
    assert!(outcome.removed_files.contains(&"my pick.dobj".to_string()));
    let objects = outcome
        .objects
        .iter()
        .map(|object| (object.file_name.as_str(), object.status))
        .collect::<HashMap<_, _>>();
    assert_eq!(objects.get("my pick.dobj"), Some(&ObjectStatus::Live));
    assert_eq!(
        objects.get(spent_name.as_str()),
        Some(&ObjectStatus::Nullified)
    );

    let landed = fx.driver.get_transaction(&tx_final_of(&record)).unwrap();
    assert_eq!(landed.output_files[0], "my pick.dobj");
    assert_eq!(landed.output_files[1], record.output_files[1]);
    assert!(landed.pending_names.is_empty());

    // Reconciling the landed record again must not spend the new version,
    // which now has the input's name.
    fx.driver
        .prune_transactions(landed.settled_at_ms.unwrap())
        .unwrap();
    assert_eq!(fx.stored("my pick.dobj").status, FileStatus::Live);
    assert!(fx.paths.objects_dir.join("my pick.dobj").exists());
}

#[test]
fn an_expired_or_deleted_mutation_leaves_the_custom_named_object_as_it_was() {
    let fx = pick_fixture("my pick.dobj");
    let old_pick = fx.stored("my pick.dobj");
    let record = fx
        .driver
        .execute(fx.input("MineStoneWithWoodPick"))
        .unwrap();
    fx.advance_head_to(record.grounding_block + MAX_STATE_ROOT_AGE_BLOCKS + 1);
    let outcome = fx.driver.settle().unwrap();
    assert_eq!(outcome.records[0].state, TransactionState::Expired);
    assert_eq!(outcome.removed_files, record.output_files);
    assert_eq!(outcome.removed_files[0], "my pick-pending.dobj");
    assert!(!fx.paths.objects_dir.join("my pick-pending.dobj").exists());
    let kept = fx.stored("my pick.dobj");
    assert_eq!(kept.content_hash, old_pick.content_hash);
    assert_eq!(kept.status, FileStatus::Live);

    let fx = pick_fixture("my pick.dobj");
    let proved = fx
        .driver
        .prove(
            fx.input("MineStoneWithWoodPick"),
            "unsent",
            &NoopExecutionReporter,
        )
        .unwrap();
    let record = fx.driver.record(proved).unwrap();
    let deleted = fx.driver.delete_transaction(&tx_final_of(&record)).unwrap();
    assert_eq!(deleted.removed_files, record.output_files);
    assert_eq!(fx.stored("my pick.dobj").status, FileStatus::Live);
}

#[test]
fn a_mutated_object_with_its_generated_name_gets_a_generated_name() {
    let fx = pick_fixture("my pick.dobj");
    let pick = fx.stored("my pick.dobj");
    let generated = object_file_name(&pick.class, &pick.obj).unwrap();
    fx.driver
        .rename_object(std::path::Path::new("my pick.dobj"), &generated)
        .unwrap();
    let input = ExecuteActionInput {
        action: craft_basics("MineStoneWithWoodPick"),
        input_objects: vec![generated.clone()],
        args: Default::default(),
    };
    let record = fx.driver.execute(input).unwrap();
    assert!(record.pending_names.is_empty());
    assert!(record.output_files[0].starts_with("craft-basics__woodpick_"));
    assert_ne!(record.output_files[0], generated);
}

#[test]
fn only_the_mutated_output_takes_a_pending_name() {
    let fx = pick_fixture("my pick.dobj");
    let record = fx
        .driver
        .execute(fx.input("MineStoneWithWoodPick"))
        .unwrap();
    assert_eq!(record.output_files[0], "my pick-pending.dobj");
    assert!(record.output_files[1].starts_with("craft-basics__stone_"));
    assert_eq!(record.pending_names.len(), 1);
}

#[test]
fn a_pending_name_skips_past_files_that_have_it() {
    let fx = pick_fixture("my pick.dobj");
    std::fs::create_dir_all(&fx.paths.discarded_objects_dir).unwrap();
    std::fs::write(
        fx.paths.discarded_objects_dir.join("my pick-pending.dobj"),
        "{}",
    )
    .unwrap();
    let record = fx
        .driver
        .execute(fx.input("MineStoneWithWoodPick"))
        .unwrap();
    assert_eq!(record.output_files[0], "my pick-pending 2.dobj");
}

#[test]
fn renaming_an_object_checks_the_name_and_what_holds_the_object() {
    let fx = pick_fixture("pick.dobj");
    let rename = |from: &str, to: &str| {
        fx.driver
            .rename_object(std::path::Path::new(from), to)
            .map(|summary| summary.file_name)
            .map_err(|err| err.to_string())
    };
    assert_eq!(rename("pick.dobj", "best pick").unwrap(), "best pick.dobj");
    assert!(!fx.paths.objects_dir.join("pick.dobj").exists());
    assert_eq!(
        rename("best pick.dobj", "Best Pick.dobj").unwrap(),
        "Best Pick.dobj"
    );
    assert!(fx.paths.objects_dir.join("Best Pick.dobj").exists());
    assert_eq!(
        rename("Best Pick.dobj", "Best Pick.dobj").unwrap(),
        "Best Pick.dobj"
    );

    for refused in ["../pick.dobj", ".pick", "", "pick?.dobj", "NUL"] {
        let err = rename("Best Pick.dobj", refused).unwrap_err();
        assert!(err.contains("invalid input"), "{refused:?}: {err}");
    }
    let err = rename(
        "Best Pick.dobj",
        "craft-basics__woodpick_0123456789abcdef_0123456789abcdef.dobj",
    )
    .unwrap_err();
    assert!(err.contains("generated object name"), "{err}");
    std::fs::create_dir_all(&fx.paths.discarded_objects_dir).unwrap();
    std::fs::write(fx.paths.discarded_objects_dir.join("taken.dobj"), "{}").unwrap();
    let err = rename("Best Pick.dobj", "taken").unwrap_err();
    assert!(err.contains("already exists"), "{err}");
    let err = rename("missing.dobj", "anything").unwrap_err();
    assert!(err.contains("missing.dobj"), "{err}");

    let input = ExecuteActionInput {
        action: craft_basics("MineStoneWithWoodPick"),
        input_objects: vec!["Best Pick.dobj".to_string()],
        args: Default::default(),
    };
    let proved = fx
        .driver
        .prove(input, "holder", &NoopExecutionReporter)
        .unwrap();
    let err = rename("Best Pick.dobj", "other").unwrap_err();
    assert!(err.contains("held by holder"), "{err}");
    let record = fx.driver.record(proved).unwrap();
    let err = rename("Best Pick.dobj", "other").unwrap_err();
    assert!(err.contains("spent by a transaction"), "{err}");
    let err = rename(&record.output_files[0], "other").unwrap_err();
    assert!(err.contains("created by a transaction"), "{err}");
}

#[test]
fn an_object_seen_spent_on_chain_moves_to_its_generated_name() {
    let fx = pick_fixture("my pick.dobj");
    let pick = fx.stored("my pick.dobj");
    fx.chain
        .lock()
        .unwrap()
        .nullifiers
        .insert(txlib::object_nullifier_hash(&pick.obj).unwrap());
    let synced = fx.driver.sync_objects(None).unwrap();
    let spent_name = object_file_name(&pick.class, &pick.obj).unwrap();
    assert_eq!(synced.outcome.removed_files, vec!["my pick.dobj"]);
    assert!(!fx.paths.objects_dir.join("my pick.dobj").exists());
    assert_eq!(fx.stored(&spent_name).status, FileStatus::Nullified);
    assert!(
        synced.objects.iter().any(
            |object| object.file_name == spent_name && object.status == ObjectStatus::Nullified
        )
    );
}
