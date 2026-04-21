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
use crate::object_record::{ObjectRecord, ObjectStatus, ensure_extra_pod_deserializers_registered};
use crate::object_store::{
    ObjectFileEntry, ensure_store_dirs, load_object_files, write_object_file,
};
use crate::pexe_catalog::{PexeCatalog, test_plugin_bytes};
use crate::{ActionQuery, DriverPaths, ExecuteActionInput, ObjectSelector};

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

fn make_input_record(file_name: &str) -> (ObjectFileEntry, DriverDeps) {
    ensure_extra_pod_deserializers_registered();
    let catalog = make_catalog();
    let outputs = catalog
        .execute_action("FindLog".to_string(), dummy_grounding_witness(), vec![])
        .unwrap();
    let spendable = outputs.obj(0);
    let id = format!("{:#}", spendable.obj.commitment());
    let record = ObjectRecord {
        id,
        class_name: "Log".to_string(),
        status: ObjectStatus::Live,
        tx_hash: None,
        pod: spendable.pod,
        obj: spendable.obj,
        tx: spendable.tx,
    };
    let mut state = TestState::default();
    apply_tx(&mut state, &record.tx);
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
        Ok(SynchronizerMembership {
            grounded_txs: HashSet::new(),
            on_chain_nullifiers: HashSet::new(),
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
    ) -> Result<relayer::api_types::SubmitProofResponse> {
        if self.fail_submit {
            return Err(anyhow!("relayer submit failed"));
        }
        Ok(relayer::api_types::SubmitProofResponse {
            job_id: "job-1".to_string(),
            status: relayer::api_types::JobStatus::Queued,
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
    let driver = Driver::open_default().unwrap();
    let filtered = driver
        .list_actions(Some(&ActionQuery {
            input_class: Some("Wood".to_string()),
            ..ActionQuery::default()
        }))
        .unwrap();
    assert!(
        filtered
            .iter()
            .all(|action| action.input_classes.contains(&"Wood".to_string()))
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
            action_id: "CraftWood".to_string(),
            input_objects: vec![ObjectSelector::FileName("log_1.dobj".to_string())],
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
            action_id: "CraftWood".to_string(),
            input_objects: vec![ObjectSelector::FileName("log_1.dobj".to_string())],
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
