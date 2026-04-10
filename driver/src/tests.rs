use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Result, anyhow};
use common::test_state::TestState;
use craft_sdk::{Helper, SpendableObject, SpendableObjects};
use pod2::middleware::Hash;
use tempfile::tempdir;
use txlib::{GroundingWitness, StateRoot};

use pod2::middleware::{Key, Value};

use crate::builtin::{actions, dependencies};
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
use crate::{ActionQuery, BuiltinActionCatalog, DriverPaths, ExecuteActionInput, ObjectSelector};

fn temp_paths() -> DriverPaths {
    let dir = tempdir().unwrap();
    let root = dir.keep();
    let settings_path = root.join("settings.json");
    let objects_dir = root.join("objects");
    let nullified_objects_dir = objects_dir.join(".nullified");
    DriverPaths {
        settings_path,
        objects_dir,
        nullified_objects_dir,
    }
}

fn make_catalog() -> MockCatalog {
    MockCatalog {
        inner: BuiltinActionCatalog::new(),
        mock_proofs: true,
    }
}

fn dummy_grounding_witness() -> GroundingWitness {
    GroundingWitness::new(
        StateRoot::new(
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
    let (transactions_root, nullifiers_root, gsrs_root, public_objects_root) = state.roots();
    StateRoot::new(
        state.block_number,
        transactions_root,
        nullifiers_root,
        gsrs_root,
        public_objects_root,
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

struct MockCatalog {
    inner: BuiltinActionCatalog,
    mock_proofs: bool,
}

impl ActionCatalog for MockCatalog {
    fn list_actions(&self) -> Vec<crate::ActionSummary> {
        self.inner.list_actions()
    }

    fn get_action(&self, action_id: &str) -> Option<crate::ActionSummary> {
        self.inner.get_action(action_id)
    }

    fn list_classes(&self) -> Vec<crate::catalog::CatalogClass> {
        self.inner.list_classes()
    }

    fn get_class(&self, class_name: &str) -> Option<crate::catalog::CatalogClass> {
        self.inner.get_class(class_name)
    }

    fn execute_action(
        &self,
        action_id: String,
        grounding_witness: GroundingWitness,
        inputs: Vec<SpendableObject>,
    ) -> Result<SpendableObjects> {
        let helper = Helper::new(dependencies(), actions());
        let builder = helper.builder(self.mock_proofs, Arc::new(grounding_witness));
        Ok(builder.action(&action_id, inputs))
    }

    fn generated_podlang(&self) -> Option<String> {
        self.inner.generated_podlang()
    }
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

/// End-to-end test for message passing: Alice creates a counter with an inbox,
/// Bob sends an "increment" message, Alice processes it and the counter goes
/// from 0 to 1. Verifies inbox_id linking, messages_root hash chain,
/// processed_count/processed_messages_root tracking, and state_commitment.
#[test]
fn test_message_passing_counter() {
    ensure_extra_pod_deserializers_registered();
    let catalog = make_catalog();

    // ---------------------------------------------------------------
    // Step 1: Alice creates the counter inbox
    // ---------------------------------------------------------------
    let create_outputs = catalog
        .execute_action(
            "CreateCounterInbox".to_string(),
            dummy_grounding_witness(),
            vec![],
        )
        .unwrap();

    // CreateCounterInbox outputs: [0] = Counter (private), [1] = Inbox (public)
    let counter = create_outputs.obj(0);
    let inbox = create_outputs.obj(1);

    // Verify counter starts at 0
    assert_eq!(
        counter.obj.get(&Key::from("count")).unwrap().unwrap().as_int().unwrap(),
        0
    );

    // Verify inbox_id links inbox and counter
    let inbox_id = inbox.obj.get(&Key::from("inbox_id")).unwrap().unwrap();
    let counter_inbox_id = counter.obj.get(&Key::from("inbox_id")).unwrap().unwrap();
    assert_eq!(inbox_id, counter_inbox_id, "inbox_id must match between inbox and counter");

    // Verify inbox initial state
    assert_eq!(
        inbox.obj.get(&Key::from("message_count")).unwrap().unwrap().as_int().unwrap(),
        0
    );
    assert_eq!(
        inbox.obj.get(&Key::from("processed_count")).unwrap().unwrap().as_int().unwrap(),
        0
    );

    // messages_root and processed_messages_root start as EMPTY_HASH
    let empty_root = Value::from(pod2::middleware::EMPTY_HASH);
    assert_eq!(
        inbox.obj.get(&Key::from("messages_root")).unwrap().unwrap(),
        empty_root,
    );
    assert_eq!(
        inbox.obj.get(&Key::from("processed_messages_root")).unwrap().unwrap(),
        empty_root,
    );

    // state_commitment = H(counter)
    let state_commitment = inbox.obj.get(&Key::from("state_commitment")).unwrap().unwrap();
    assert_eq!(
        state_commitment,
        Value::from(pod2::middleware::RawValue::from(counter.obj.commitment()))
    );

    // Apply create tx to canonical state
    let mut state = TestState::default();
    apply_tx(&mut state, &create_outputs.tx);

    // ---------------------------------------------------------------
    // Step 2: Bob sends an "increment" message
    // ---------------------------------------------------------------
    let bob_witness = GroundingWitness::new(
        state_root(&state),
        [(tx_hash(&inbox.tx), state.tx_membership_proof(tx_hash(&inbox.tx)))]
            .into_iter()
            .collect(),
    );

    let send_outputs = catalog
        .execute_action(
            "SendCounterMessage".to_string(),
            bob_witness,
            vec![inbox.clone()],
        )
        .unwrap();

    // SendCounterMessage outputs: [0] = InboxMessage, [1] = new Inbox
    let message = send_outputs.obj(0);
    let updated_inbox = send_outputs.obj(1);

    // Verify message has amount=1 and correct inbox_id
    assert_eq!(message.obj.get(&Key::from("amount")).unwrap().unwrap().as_int().unwrap(), 1);
    assert_eq!(
        message.obj.get(&Key::from("inbox_id")).unwrap().unwrap(),
        inbox_id,
        "message inbox_id must match"
    );

    // Verify inbox message_count incremented
    assert_eq!(
        updated_inbox.obj.get(&Key::from("message_count")).unwrap().unwrap().as_int().unwrap(),
        1
    );

    // Verify messages_root = H(EMPTY_HASH, commitment(message))
    let expected_root = pod2::middleware::hash_values(&[
        Value::from(pod2::middleware::EMPTY_HASH),
        Value::from(message.obj.commitment()),
    ]);
    assert_eq!(
        updated_inbox.obj.get(&Key::from("messages_root")).unwrap().unwrap(),
        Value::from(pod2::middleware::RawValue::from(expected_root)),
        "messages_root must be H(old_root, commitment(message))"
    );

    // Verify processing fields NOT changed by sender
    assert_eq!(
        updated_inbox.obj.get(&Key::from("processed_count")).unwrap().unwrap().as_int().unwrap(),
        0,
        "sender must not touch processed_count"
    );
    assert_eq!(
        updated_inbox.obj.get(&Key::from("processed_messages_root")).unwrap().unwrap(),
        empty_root,
        "sender must not touch processed_messages_root"
    );
    assert_eq!(
        updated_inbox.obj.get(&Key::from("state_commitment")).unwrap().unwrap(),
        state_commitment,
        "sender must not touch state_commitment"
    );
    assert_eq!(
        updated_inbox.obj.get(&Key::from("inbox_id")).unwrap().unwrap(),
        inbox_id,
        "sender must not touch inbox_id"
    );

    // Apply send tx to canonical state
    apply_tx(&mut state, &send_outputs.tx);

    // ---------------------------------------------------------------
    // Step 3: Alice processes the message
    // ---------------------------------------------------------------
    let alice_witness = GroundingWitness::new(
        state_root(&state),
        [
            (tx_hash(&message.tx), state.tx_membership_proof(tx_hash(&message.tx))),
            (tx_hash(&updated_inbox.tx), state.tx_membership_proof(tx_hash(&updated_inbox.tx))),
            (tx_hash(&counter.tx), state.tx_membership_proof(tx_hash(&counter.tx))),
        ]
        .into_iter()
        .collect(),
    );

    let process_outputs = catalog
        .execute_action(
            "ProcessCounterMessages".to_string(),
            alice_witness,
            vec![
                message.clone(),
                updated_inbox.clone(),
                counter.clone(),
            ],
        )
        .unwrap();

    // ProcessCounterMessages outputs: [0] = new Counter, [1] = new Inbox
    let final_counter = process_outputs.obj(0);
    let final_inbox = process_outputs.obj(1);

    // Verify counter is now 1
    assert_eq!(
        final_counter.obj.get(&Key::from("count")).unwrap().unwrap().as_int().unwrap(),
        1,
        "counter should be 1 after processing increment"
    );

    // Verify counter preserves inbox_id
    assert_eq!(
        final_counter.obj.get(&Key::from("inbox_id")).unwrap().unwrap(),
        inbox_id,
        "counter inbox_id must be preserved"
    );

    // Verify inbox state_commitment matches new counter
    assert_eq!(
        final_inbox.obj.get(&Key::from("state_commitment")).unwrap().unwrap(),
        Value::from(pod2::middleware::RawValue::from(final_counter.obj.commitment())),
        "state_commitment should match the updated counter"
    );

    // Verify processed_count caught up to message_count
    assert_eq!(
        final_inbox.obj.get(&Key::from("processed_count")).unwrap().unwrap().as_int().unwrap(),
        1,
        "processed_count should catch up to message_count"
    );

    // Verify processed_messages_root caught up to messages_root
    assert_eq!(
        final_inbox.obj.get(&Key::from("processed_messages_root")).unwrap().unwrap(),
        final_inbox.obj.get(&Key::from("messages_root")).unwrap().unwrap(),
        "processed_messages_root should catch up to messages_root"
    );

    // Verify message_count and messages_root preserved through processing
    assert_eq!(
        final_inbox.obj.get(&Key::from("message_count")).unwrap().unwrap().as_int().unwrap(),
        1,
        "message_count must be preserved during processing"
    );
    assert_eq!(
        final_inbox.obj.get(&Key::from("messages_root")).unwrap().unwrap(),
        Value::from(pod2::middleware::RawValue::from(expected_root)),
        "messages_root must be preserved during processing"
    );

    // Verify inbox_id preserved through processing
    assert_eq!(
        final_inbox.obj.get(&Key::from("inbox_id")).unwrap().unwrap(),
        inbox_id,
        "inbox_id must be preserved during processing"
    );

    println!("Message passing test passed:");
    println!("  - inbox_id links inbox and counter");
    println!("  - messages_root hash chain: H(EMPTY, commitment(msg)) verified");
    println!("  - processed_count/processed_messages_root caught up after processing");
    println!("  - state_commitment updated to H(counter_at_1)");
    println!("  - counter: 0 -> 1 via message amount=1");
}

/// Test MessageRejected: Alice creates a counter at 0, someone sends a
/// negative amount message (-5), Alice rejects it because it would make
/// the counter negative. The inbox advances (processed_count, hash chain)
/// but state_commitment stays the same. Counter is untouched.
#[test]
fn test_message_passing_reject() {
    ensure_extra_pod_deserializers_registered();
    let catalog = make_catalog();

    // Step 1: Create counter inbox (counter starts at 0)
    let create_outputs = catalog
        .execute_action(
            "CreateCounterInbox".to_string(),
            dummy_grounding_witness(),
            vec![],
        )
        .unwrap();

    let counter = create_outputs.obj(0);
    let inbox = create_outputs.obj(1);
    let inbox_id = inbox.obj.get(&Key::from("inbox_id")).unwrap().unwrap();
    let state_commitment_before = inbox.obj.get(&Key::from("state_commitment")).unwrap().unwrap();

    let mut state = TestState::default();
    apply_tx(&mut state, &create_outputs.tx);

    // Step 2: Someone sends a negative amount (-5)
    let send_witness = GroundingWitness::new(
        state_root(&state),
        [(tx_hash(&inbox.tx), state.tx_membership_proof(tx_hash(&inbox.tx)))]
            .into_iter()
            .collect(),
    );

    let send_outputs = catalog
        .execute_action(
            "SendNegativeCounterMessage".to_string(),
            send_witness,
            vec![inbox.clone()],
        )
        .unwrap();

    let bad_message = send_outputs.obj(0);
    let updated_inbox = send_outputs.obj(1);

    // Verify message has amount=-5
    assert_eq!(
        bad_message.obj.get(&Key::from("amount")).unwrap().unwrap().as_int().unwrap(),
        -5
    );

    // Verify inbox message_count is 1
    assert_eq!(
        updated_inbox.obj.get(&Key::from("message_count")).unwrap().unwrap().as_int().unwrap(),
        1
    );

    // processed_count is still 0 (nothing processed yet)
    assert_eq!(
        updated_inbox.obj.get(&Key::from("processed_count")).unwrap().unwrap().as_int().unwrap(),
        0
    );

    apply_tx(&mut state, &send_outputs.tx);

    // Step 3: Alice rejects the message — counter is consumed and
    // re-emitted unchanged so the proof can read count and prove
    // count + amount < 0.
    let reject_witness = GroundingWitness::new(
        state_root(&state),
        [
            (tx_hash(&bad_message.tx), state.tx_membership_proof(tx_hash(&bad_message.tx))),
            (tx_hash(&updated_inbox.tx), state.tx_membership_proof(tx_hash(&updated_inbox.tx))),
            (tx_hash(&counter.tx), state.tx_membership_proof(tx_hash(&counter.tx))),
        ]
        .into_iter()
        .collect(),
    );

    let reject_outputs = catalog
        .execute_action(
            "RejectCounterMessage".to_string(),
            reject_witness,
            vec![
                bad_message.clone(),
                updated_inbox.clone(),
                counter.clone(),
            ],
        )
        .unwrap();

    // RejectCounterMessage outputs: [0] = new Counter (same count), [1] = new Inbox
    let rejected_counter = reject_outputs.obj(0);
    let rejected_inbox = reject_outputs.obj(1);

    // Verify counter is UNCHANGED (still 0)
    assert_eq!(
        rejected_counter.obj.get(&Key::from("count")).unwrap().unwrap().as_int().unwrap(),
        0,
        "counter must stay at 0 after rejection"
    );

    // Verify processed_count advanced to 1 (message was processed as rejected)
    assert_eq!(
        rejected_inbox.obj.get(&Key::from("processed_count")).unwrap().unwrap().as_int().unwrap(),
        1,
        "processed_count should advance even on rejection"
    );

    // Verify state_commitment is UNCHANGED (counter wasn't modified)
    assert_eq!(
        rejected_inbox.obj.get(&Key::from("state_commitment")).unwrap().unwrap(),
        state_commitment_before,
        "state_commitment must not change on rejection"
    );

    // Verify message_count preserved
    assert_eq!(
        rejected_inbox.obj.get(&Key::from("message_count")).unwrap().unwrap().as_int().unwrap(),
        1,
        "message_count must be preserved during rejection"
    );

    // Verify messages_root preserved
    assert_eq!(
        rejected_inbox.obj.get(&Key::from("messages_root")).unwrap().unwrap(),
        updated_inbox.obj.get(&Key::from("messages_root")).unwrap().unwrap(),
        "messages_root must be preserved during rejection"
    );

    // Verify processed_messages_root advanced (chain link consumed)
    let rejected_pmr = rejected_inbox.obj.get(&Key::from("processed_messages_root")).unwrap().unwrap();
    let expected_pmr = pod2::middleware::hash_values(&[
        Value::from(pod2::middleware::EMPTY_HASH),
        Value::from(bad_message.obj.commitment()),
    ]);
    assert_eq!(
        rejected_pmr,
        Value::from(pod2::middleware::RawValue::from(expected_pmr)),
        "processed_messages_root should advance by one chain link"
    );

    // Verify processed_messages_root caught up to messages_root (fully processed)
    assert_eq!(
        rejected_pmr,
        rejected_inbox.obj.get(&Key::from("messages_root")).unwrap().unwrap(),
        "should be fully caught up after rejecting the only message"
    );

    // Verify inbox_id preserved
    assert_eq!(
        rejected_inbox.obj.get(&Key::from("inbox_id")).unwrap().unwrap(),
        inbox_id,
        "inbox_id must be preserved during rejection"
    );

    println!("Message rejection test passed:");
    println!("  - SendNegativeCounterMessage: amount=-5, message_count=1");
    println!("  - RejectCounterMessage: processed_count=1, state_commitment unchanged");
    println!("  - Hash chain advanced, counter untouched");
}

/// Verify that Dictionary commitment survives JSON round-trip.
#[test]
fn test_dictionary_commitment_json_roundtrip() {
    use pod2::middleware::containers::Dictionary;
    use std::collections::HashMap as StdMap;

    let mut map = StdMap::new();
    map.insert(Key::from("blueprint"), Value::from("InboxMessage"));
    map.insert(Key::from("public"), Value::from(true));
    map.insert(Key::from("amount"), Value::from(1i64));
    map.insert(Key::from("inbox_id"), Value::from(42i64));
    map.insert(Key::from("key"), Value::from(pod2utils::rand_raw_value()));
    map.insert(Key::from("work"), Value::from(pod2::middleware::EMPTY_VALUE));
    let dict = Dictionary::new(map);

    let commitment_before = dict.commitment();

    let json = serde_json::to_string(&dict).unwrap();
    let dict2: Dictionary = serde_json::from_str(&json).unwrap();
    let commitment_after = dict2.commitment();

    assert_eq!(
        commitment_before, commitment_after,
        "Dictionary commitment must survive JSON round-trip"
    );
    println!("Dictionary commitment round-trip: OK ({:#})", commitment_before);
}

/// Reproduce the GUI's HashOf error: simulate disk round-trip of
/// objects between SendCounterMessage and ProcessCounterMessages.
#[test]
fn test_message_passing_counter_with_roundtrip() {
    use pod2::middleware::containers::Dictionary;

    ensure_extra_pod_deserializers_registered();
    let catalog = make_catalog();

    // Step 1: Create
    let create_outputs = catalog
        .execute_action("CreateCounterInbox".to_string(), dummy_grounding_witness(), vec![])
        .unwrap();
    let counter = create_outputs.obj(0);
    let inbox = create_outputs.obj(1);

    let mut state = TestState::default();
    apply_tx(&mut state, &create_outputs.tx);

    // Step 2: Send
    let bob_witness = GroundingWitness::new(
        state_root(&state),
        [(tx_hash(&inbox.tx), state.tx_membership_proof(tx_hash(&inbox.tx)))]
            .into_iter()
            .collect(),
    );
    let send_outputs = catalog
        .execute_action("SendCounterMessage".to_string(), bob_witness, vec![inbox.clone()])
        .unwrap();
    let message = send_outputs.obj(0);
    let updated_inbox = send_outputs.obj(1);

    // Simulate disk round-trip: serialize to JSON and back (like .dobj files)
    let msg_json = serde_json::to_string(&message.obj).unwrap();
    let msg_obj_roundtrip: Dictionary = serde_json::from_str(&msg_json).unwrap();
    println!(
        "message commitment before: {:#}, after: {:#}, equal: {}",
        message.obj.commitment(),
        msg_obj_roundtrip.commitment(),
        message.obj.commitment() == msg_obj_roundtrip.commitment()
    );

    let inbox_json = serde_json::to_string(&updated_inbox.obj).unwrap();
    let inbox_obj_roundtrip: Dictionary = serde_json::from_str(&inbox_json).unwrap();

    // Verify the hash chain relationship holds after round-trip
    let messages_root = inbox_obj_roundtrip.get(&Key::from("messages_root")).unwrap().unwrap();
    let processed_messages_root = inbox_obj_roundtrip.get(&Key::from("processed_messages_root")).unwrap().unwrap();
    let expected = pod2::middleware::hash_values(&[
        processed_messages_root.clone(),
        Value::from(msg_obj_roundtrip.commitment()),
    ]);
    println!(
        "messages_root: {:#}, H(pmr, msg_commit): {:#}, equal: {}",
        messages_root.raw(),
        pod2::middleware::RawValue::from(expected),
        messages_root == Value::from(pod2::middleware::RawValue::from(expected))
    );

    apply_tx(&mut state, &send_outputs.tx);

    // Step 3: Process using round-tripped objects
    let message_rt = SpendableObject {
        pod: message.pod.clone(),
        obj: msg_obj_roundtrip,
        tx: message.tx.clone(),
    };
    let inbox_rt = SpendableObject {
        pod: updated_inbox.pod.clone(),
        obj: inbox_obj_roundtrip,
        tx: updated_inbox.tx.clone(),
    };
    let counter_json = serde_json::to_string(&counter.obj).unwrap();
    let counter_obj_roundtrip: Dictionary = serde_json::from_str(&counter_json).unwrap();
    let counter_rt = SpendableObject {
        pod: counter.pod.clone(),
        obj: counter_obj_roundtrip,
        tx: counter.tx.clone(),
    };

    let alice_witness = GroundingWitness::new(
        state_root(&state),
        [
            (tx_hash(&message.tx), state.tx_membership_proof(tx_hash(&message.tx))),
            (tx_hash(&updated_inbox.tx), state.tx_membership_proof(tx_hash(&updated_inbox.tx))),
            (tx_hash(&counter.tx), state.tx_membership_proof(tx_hash(&counter.tx))),
        ]
        .into_iter()
        .collect(),
    );

    let process_outputs = catalog
        .execute_action(
            "ProcessCounterMessages".to_string(),
            alice_witness,
            vec![message_rt, inbox_rt, counter_rt],
        )
        .unwrap();

    let final_counter = process_outputs.obj(0);
    assert_eq!(
        final_counter.obj.get(&Key::from("count")).unwrap().unwrap().as_int().unwrap(),
        1,
        "counter should be 1 after round-trip processing"
    );
    println!("Round-trip test passed: counter = 1");
}

/// Test ProcessAndRejectCounterMessages: Alice creates a counter at 0,
/// Bob sends an increment (+1), then someone sends a negative (-5).
/// Alice calls ProcessAndRejectCounterMessages to apply msg1 and reject msg2
/// in one batch. Counter ends at 1 (only msg1 applied), inbox fully caught up.
#[test]
fn test_process_and_reject_counter_messages() {
    ensure_extra_pod_deserializers_registered();
    let catalog = make_catalog();

    // Step 1: Create counter inbox (counter starts at 0)
    let create_outputs = catalog
        .execute_action(
            "CreateCounterInbox".to_string(),
            dummy_grounding_witness(),
            vec![],
        )
        .unwrap();

    let counter = create_outputs.obj(0);
    let inbox = create_outputs.obj(1);
    let inbox_id = inbox.obj.get(&Key::from("inbox_id")).unwrap().unwrap();

    let mut state = TestState::default();
    apply_tx(&mut state, &create_outputs.tx);

    // Step 2: Bob sends +1
    let send_witness = GroundingWitness::new(
        state_root(&state),
        [(tx_hash(&inbox.tx), state.tx_membership_proof(tx_hash(&inbox.tx)))]
            .into_iter()
            .collect(),
    );

    let send_outputs = catalog
        .execute_action(
            "SendCounterMessage".to_string(),
            send_witness,
            vec![inbox.clone()],
        )
        .unwrap();

    let message1 = send_outputs.obj(0);
    let inbox_after_msg1 = send_outputs.obj(1);

    assert_eq!(
        message1.obj.get(&Key::from("amount")).unwrap().unwrap().as_int().unwrap(),
        1
    );

    apply_tx(&mut state, &send_outputs.tx);

    // Step 3: Someone sends -5
    let send2_witness = GroundingWitness::new(
        state_root(&state),
        [(tx_hash(&inbox_after_msg1.tx), state.tx_membership_proof(tx_hash(&inbox_after_msg1.tx)))]
            .into_iter()
            .collect(),
    );

    let send2_outputs = catalog
        .execute_action(
            "SendNegativeCounterMessage".to_string(),
            send2_witness,
            vec![inbox_after_msg1.clone()],
        )
        .unwrap();

    let message2 = send2_outputs.obj(0);
    let inbox_after_msg2 = send2_outputs.obj(1);

    assert_eq!(
        message2.obj.get(&Key::from("amount")).unwrap().unwrap().as_int().unwrap(),
        -5
    );
    assert_eq!(
        inbox_after_msg2.obj.get(&Key::from("message_count")).unwrap().unwrap().as_int().unwrap(),
        2
    );

    apply_tx(&mut state, &send2_outputs.tx);

    // Step 4: Alice processes msg1 and rejects msg2 in one batch
    let pr_witness = GroundingWitness::new(
        state_root(&state),
        [
            (tx_hash(&message1.tx), state.tx_membership_proof(tx_hash(&message1.tx))),
            (tx_hash(&message2.tx), state.tx_membership_proof(tx_hash(&message2.tx))),
            (tx_hash(&inbox_after_msg2.tx), state.tx_membership_proof(tx_hash(&inbox_after_msg2.tx))),
            (tx_hash(&counter.tx), state.tx_membership_proof(tx_hash(&counter.tx))),
        ]
        .into_iter()
        .collect(),
    );

    let pr_outputs = catalog
        .execute_action(
            "ProcessAndRejectCounterMessages".to_string(),
            pr_witness,
            vec![
                message1.clone(),
                message2.clone(),
                inbox_after_msg2.clone(),
                counter.clone(),
            ],
        )
        .unwrap();

    let final_counter = pr_outputs.obj(0);
    let final_inbox = pr_outputs.obj(1);

    // Counter should be 1 (only msg1 applied, msg2 rejected)
    assert_eq!(
        final_counter.obj.get(&Key::from("count")).unwrap().unwrap().as_int().unwrap(),
        1,
        "counter must be 1 (msg1 +1 applied, msg2 -5 rejected)"
    );

    // processed_count should be 2 (both messages consumed from queue)
    assert_eq!(
        final_inbox.obj.get(&Key::from("processed_count")).unwrap().unwrap().as_int().unwrap(),
        2,
        "processed_count must be 2"
    );

    // Fully caught up: processed_messages_root == messages_root
    let final_pmr = final_inbox.obj.get(&Key::from("processed_messages_root")).unwrap().unwrap();
    let final_mr = final_inbox.obj.get(&Key::from("messages_root")).unwrap().unwrap();
    assert_eq!(
        final_pmr, final_mr,
        "must be fully caught up after processing both messages"
    );

    // Verify hash chain: H(H(EMPTY, c1), c2) == messages_root
    let expected_mid = pod2::middleware::hash_values(&[
        Value::from(pod2::middleware::EMPTY_HASH),
        Value::from(message1.obj.commitment()),
    ]);
    let expected_final = pod2::middleware::hash_values(&[
        Value::from(expected_mid),
        Value::from(message2.obj.commitment()),
    ]);
    assert_eq!(
        final_mr,
        Value::from(pod2::middleware::RawValue::from(expected_final)),
        "hash chain must match"
    );

    // state_commitment should reflect counter AFTER msg1 (count=1), not unchanged
    let new_state_commitment = final_inbox.obj.get(&Key::from("state_commitment")).unwrap().unwrap();
    let expected_sc = Value::from(pod2::middleware::RawValue::from(final_counter.obj.commitment()));
    assert_eq!(
        new_state_commitment, expected_sc,
        "state_commitment must reflect counter after applying msg1"
    );

    // inbox_id preserved
    assert_eq!(
        final_inbox.obj.get(&Key::from("inbox_id")).unwrap().unwrap(),
        inbox_id,
        "inbox_id must be preserved"
    );

    println!("ProcessAndRejectCounterMessages test passed:");
    println!("  - msg1 (+1) applied, msg2 (-5) rejected");
    println!("  - counter: 0 -> 1");
    println!("  - processed_count: 0 -> 2, fully caught up");
    println!("  - state_commitment updated to H(counter_at_1)");
}
