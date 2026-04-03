mod builtin;
mod catalog;
mod clients;
mod object_record;
mod object_store;
mod paths;
mod runtime;
mod settings;
mod types;

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use common::{
    encode_hash_hex,
    payload::{Payload, PayloadProof},
    shrink::{ShrunkMainPodSetup, shrink_compress_pod},
};
use craft_sdk::SpendableObjects;
use pod2::middleware::{Hash, Params};
use txlib::object_nullifier_hash;

use crate::catalog::{ActionCatalog, CatalogClass};
use crate::clients::{
    HttpRelayerClient, HttpSynchronizerClient, RELAYER_POLL_INTERVAL_MS,
    RELAYER_POLL_TIMEOUT_SECS, RelayerClient, SYNCHRONIZER_POLL_INTERVAL_MS,
    SYNCHRONIZER_POLL_TIMEOUT_SECS, SynchronizerClient,
};
use crate::object_record::ObjectRecord;
use crate::object_store::{
    ObjectFileEntry, ensure_store_dirs, load_object_files, matches_query, parse_object_file_from_path,
    select_object, write_object_file,
};
use crate::runtime::ActionRunGate;
use crate::settings::{default_settings, read_settings, write_settings};
pub use crate::types::{
    ActionQuery, ActionSummary, CheckActionCandidate, CheckActionReport, ClassSummary,
    DriverPaths, DriverSettings, ExecuteActionInput, ExecuteActionResult, ExecutionPhase,
    ExecutionReporter, NoopExecutionReporter, ObjectDetail, ObjectQuery, ObjectSelector,
    ObjectSummary,
};

pub use crate::builtin::BuiltinActionCatalog;
pub use crate::catalog::ActionCatalog as DriverActionCatalog;
pub use crate::clients::{
    RELAYER_POLL_INTERVAL_MS as DEFAULT_RELAYER_POLL_INTERVAL_MS,
    RELAYER_POLL_TIMEOUT_SECS as DEFAULT_RELAYER_POLL_TIMEOUT_SECS,
    SYNCHRONIZER_POLL_INTERVAL_MS as DEFAULT_SYNCHRONIZER_POLL_INTERVAL_MS,
    SYNCHRONIZER_POLL_TIMEOUT_SECS as DEFAULT_SYNCHRONIZER_POLL_TIMEOUT_SECS,
};

pub trait PayloadBuilder: Send + Sync {
    fn build_payload(
        &self,
        old_state_root_hash: &Hash,
        action_output: &SpendableObjects,
    ) -> Result<Vec<u8>>;
}

#[derive(Clone, Default)]
struct DefaultPayloadBuilder;

impl PayloadBuilder for DefaultPayloadBuilder {
    fn build_payload(
        &self,
        old_state_root_hash: &Hash,
        action_output: &SpendableObjects,
    ) -> Result<Vec<u8>> {
        build_relayer_payload(old_state_root_hash, action_output)
    }
}

#[derive(Clone)]
pub struct DriverDeps {
    pub catalog: Arc<dyn ActionCatalog>,
    pub synchronizer: Arc<dyn SynchronizerClient>,
    pub relayer: Arc<dyn RelayerClient>,
    pub payload_builder: Arc<dyn PayloadBuilder>,
}

impl Default for DriverDeps {
    fn default() -> Self {
        Self {
            catalog: Arc::new(BuiltinActionCatalog::new()),
            synchronizer: Arc::new(HttpSynchronizerClient),
            relayer: Arc::new(HttpRelayerClient),
            payload_builder: Arc::new(DefaultPayloadBuilder),
        }
    }
}

#[derive(Clone)]
pub struct Driver {
    paths: DriverPaths,
    deps: DriverDeps,
    run_gate: Arc<ActionRunGate>,
}

impl Driver {
    pub fn open_default() -> Result<Self> {
        Self::open(paths::default_paths()?, DriverDeps::default())
    }

    pub fn open(paths: DriverPaths, deps: DriverDeps) -> Result<Self> {
        ensure_store_dirs(&paths)?;
        Ok(Self {
            paths,
            deps,
            run_gate: Arc::new(ActionRunGate::new()),
        })
    }

    pub fn paths(&self) -> &DriverPaths {
        &self.paths
    }

    pub fn load_settings(&self) -> Result<DriverSettings> {
        if let Some(settings) = read_settings(&self.paths)? {
            return Ok(settings);
        }
        let settings = default_settings();
        write_settings(&self.paths, &settings)?;
        Ok(settings)
    }

    pub fn save_settings(&self, settings: &DriverSettings) -> Result<DriverSettings> {
        write_settings(&self.paths, settings)?;
        Ok(settings.clone())
    }

    pub fn list_objects(&self, query: Option<&ObjectQuery>) -> Result<Vec<ObjectSummary>> {
        let entries = load_object_files(&self.paths)?;
        Ok(entries
            .iter()
            .filter(|entry| query.is_none_or(|query| matches_query(entry, query)))
            .map(|entry| self.object_summary(entry, None))
            .collect())
    }

    pub fn read_object(&self, selector: &ObjectSelector) -> Result<ObjectDetail> {
        let entries = load_object_files(&self.paths)?;
        let entry = select_object(&entries, selector)?;
        Ok(self.object_detail(entry, None))
    }

    pub fn read_object_file(&self, path: &Path) -> Result<ObjectDetail> {
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| anyhow!("invalid input path (missing file name): {}", path.display()))?
            .to_string();
        let record = parse_object_file_from_path(path)?;
        Ok(self.object_detail(
            &ObjectFileEntry { file_name, record },
            None,
        ))
    }

    pub fn sync_inventory(&self, query: Option<&ObjectQuery>) -> Result<Vec<ObjectSummary>> {
        let mut entries = load_object_files(&self.paths)?;
        let settings = self.load_settings()?;
        let source_tx_hashes = entries
            .iter()
            .map(|entry| entry.record.spendable().tx.dict().commitment())
            .collect::<HashSet<_>>();
        let live_nullifiers = entries
            .iter()
            .filter(|entry| !entry.record.is_nullified())
            .filter_map(|entry| object_nullifier_hash(&entry.record.obj).ok())
            .collect::<HashSet<_>>();

        let membership = self.deps.synchronizer.fetch_membership_with_nullifiers(
            &settings.synchronizer_api_url,
            &source_tx_hashes.iter().copied().collect::<Vec<_>>(),
            &live_nullifiers.iter().copied().collect::<Vec<_>>(),
        )?;

        reconcile_objects(
            &self.paths,
            &mut entries,
            &membership.on_chain_nullifiers,
        );

        Ok(entries
            .iter()
            .filter(|entry| query.is_none_or(|query| matches_query(entry, query)))
            .map(|entry| {
                let source_tx_hash = entry.record.spendable().tx.dict().commitment();
                let grounded = entry.record.is_nullified()
                    || membership.grounded_txs.contains(&source_tx_hash);
                self.object_summary(entry, Some(grounded))
            })
            .collect())
    }

    pub fn list_actions(&self, query: Option<&ActionQuery>) -> Result<Vec<ActionSummary>> {
        let actions = self.deps.catalog.list_actions();
        Ok(actions
            .into_iter()
            .filter(|action| {
                query.is_none_or(|query| {
                    query.name
                        .as_ref()
                        .is_none_or(|name| &action.id == name)
                        && query
                            .input_class
                            .as_ref()
                            .is_none_or(|class_name| action.input_classes.contains(class_name))
                        && query
                            .output_class
                            .as_ref()
                            .is_none_or(|class_name| action.output_classes.contains(class_name))
                })
            })
            .collect())
    }

    pub fn list_classes(&self) -> Result<Vec<ClassSummary>> {
        let live_objects = load_object_files(&self.paths)?
            .into_iter()
            .filter(|entry| !entry.record.is_nullified())
            .collect::<Vec<_>>();
        Ok(self
            .deps
            .catalog
            .list_classes()
            .into_iter()
            .map(|class_info| ClassSummary {
                live_count: live_objects
                    .iter()
                    .filter(|entry| entry.record.class_name == class_info.name)
                    .count(),
                ..self.class_summary(class_info)
            })
            .collect())
    }

    pub fn get_class(&self, class_name: &str) -> Result<ClassSummary> {
        let class_info = self
            .deps
            .catalog
            .get_class(class_name)
            .ok_or_else(|| anyhow!("unknown class: {class_name}"))?;
        let live_count = load_object_files(&self.paths)?
            .into_iter()
            .filter(|entry| !entry.record.is_nullified() && entry.record.class_name == class_name)
            .count();
        Ok(ClassSummary {
            live_count,
            ..self.class_summary(class_info)
        })
    }

    pub fn check_action(&self, action_id: &str) -> Result<CheckActionReport> {
        let action = self
            .deps
            .catalog
            .get_action(action_id)
            .ok_or_else(|| anyhow!("unknown action: {action_id}"))?;
        let entries = load_object_files(&self.paths)?;
        let live_objects = entries
            .iter()
            .filter(|entry| !entry.record.is_nullified())
            .collect::<Vec<_>>();

        let mut available = Vec::new();
        let mut missing = Vec::new();
        let mut used_ids = HashSet::new();

        for required_class in &action.input_classes {
            if let Some(entry) = live_objects
                .iter()
                .find(|entry| {
                    &entry.record.class_name == required_class && !used_ids.contains(&entry.record.id)
                })
            {
                used_ids.insert(entry.record.id.clone());
                available.push(CheckActionCandidate {
                    class_name: entry.record.class_name.clone(),
                    object_id: entry.record.id.clone(),
                    file_name: entry.file_name.clone(),
                });
            } else {
                missing.push(required_class.clone());
            }
        }

        Ok(CheckActionReport {
            feasible: missing.is_empty(),
            action_id: action_id.to_string(),
            available_inputs: available,
            missing_inputs: missing,
        })
    }

    pub fn get_state_root(&self) -> Result<String> {
        let settings = self.load_settings()?;
        let head = self
            .deps
            .synchronizer
            .fetch_head(&settings.synchronizer_api_url)?;
        Ok(encode_hash_hex(&head.current_gsr))
    }

    pub fn execute(&self, input: ExecuteActionInput) -> Result<ExecuteActionResult> {
        self.execute_with_reporter(input, &NoopExecutionReporter)
    }

    pub fn execute_with_reporter<R: ExecutionReporter>(
        &self,
        input: ExecuteActionInput,
        reporter: &R,
    ) -> Result<ExecuteActionResult> {
        let _run_guard = self.run_gate.acquire()?;
        let settings = self.load_settings()?;
        let action = self
            .deps
            .catalog
            .get_action(&input.action_id)
            .ok_or_else(|| anyhow!("unknown action: {}", input.action_id))?;

        validate_execute_request(&input, &action)?;

        reporter.on_step(ExecutionPhase::GenerateProof, "Verifying Inputs");
        let entries = load_object_files(&self.paths)?;
        let resolved_inputs = resolve_inputs(&entries, &input, &action)?;
        let source_tx_hashes = resolved_inputs
            .iter()
            .map(|entry| entry.record.spendable().tx.dict().commitment())
            .collect::<Vec<_>>();
        let grounding_witness = self.deps.synchronizer.fetch_grounding_witness(
            &settings.synchronizer_api_url,
            &source_tx_hashes,
        )?;
        let old_root_hash = grounding_witness.state_root.hash();
        let old_root = encode_hash_hex(&old_root_hash);

        reporter.on_step(ExecutionPhase::GenerateProof, "Generating proof");
        let execution_inputs = resolved_inputs
            .iter()
            .map(|input| input.record.spendable())
            .collect::<Vec<_>>();
        let spendable_outputs = self.deps.catalog.execute_action(
            input.action_id.clone(),
            grounding_witness,
            execution_inputs,
        )?;
        reporter.on_done(ExecutionPhase::GenerateProof, None);

        reporter.on_step(ExecutionPhase::Commit, "Shrinking proof");
        let payload_bytes = self
            .deps
            .payload_builder
            .build_payload(&old_root_hash, &spendable_outputs)?;
        let expected_tx_final = spendable_outputs.tx.dict().commitment();

        reporter.on_step(ExecutionPhase::Commit, "Creating files");
        let saved = save_results(&self.paths, &action, &input.action_id, &resolved_inputs, &spendable_outputs)?;

        let commit_result: Result<ExecuteActionResult> = (|| {
            reporter.on_step(ExecutionPhase::Commit, "Submitting proof to relayer");
            let submit_response = self.deps.relayer.submit_proof(
                &settings.relayer_api_url,
                &payload_bytes,
                Some(format!("driver:{}", input.action_id)),
            )?;
            if submit_response.status == relayer::api_types::JobStatus::Failed {
                return Err(anyhow!(
                    "relayer rejected job {} immediately",
                    submit_response.job_id
                ));
            }

            let waiting_label = format!("Waiting for relayer job {}", submit_response.job_id);
            reporter.on_step(ExecutionPhase::Commit, &waiting_label);
            let confirmation = self.deps.relayer.wait_for_confirmation(
                &settings.relayer_api_url,
                &submit_response.job_id,
                RELAYER_POLL_TIMEOUT_SECS,
                RELAYER_POLL_INTERVAL_MS,
            )?;

            reporter.on_step(
                ExecutionPhase::Commit,
                "Waiting for synchronizer to observe commit",
            );
            let sync_head = self.deps.synchronizer.wait_for_tx(
                &settings.synchronizer_api_url,
                expected_tx_final,
                SYNCHRONIZER_POLL_TIMEOUT_SECS,
                SYNCHRONIZER_POLL_INTERVAL_MS,
            )?;
            Ok(ExecuteActionResult {
                old_root,
                new_root: encode_hash_hex(&sync_head.current_gsr),
                output_files: saved.output_files.clone(),
                nullified_files: saved.nullified_files.clone(),
                relayer_job_id: confirmation.job_id,
                tx_hash: confirmation.tx_hash,
                block_number: confirmation.block_number,
            })
        })();

        match commit_result {
            Ok(result) => {
                reporter.on_done(ExecutionPhase::Commit, Some(&result));
                Ok(result)
            }
            Err(err) => {
                rollback_results(&self.paths, &resolved_inputs, &saved);
                Err(err)
            }
        }
    }

    pub fn generated_podlang(&self) -> Option<String> {
        self.deps.catalog.generated_podlang()
    }

    fn object_summary(&self, entry: &ObjectFileEntry, grounded: Option<bool>) -> ObjectSummary {
        let class_hash = self
            .deps
            .catalog
            .get_class(&entry.record.class_name)
            .map(|class_info| class_info.hash)
            .unwrap_or_default();
        ObjectSummary {
            id: entry.record.id.clone(),
            file_name: entry.file_name.clone(),
            class_name: entry.record.class_name.clone(),
            class_hash,
            source_action: entry.record.source_action.clone(),
            live: !entry.record.is_nullified(),
            nullifier: entry.record.nullifier.clone(),
            grounded,
            fields: entry.record.fields_map(),
        }
    }

    fn object_detail(&self, entry: &ObjectFileEntry, grounded: Option<bool>) -> ObjectDetail {
        let class_info = self.deps.catalog.get_class(&entry.record.class_name);
        ObjectDetail {
            id: entry.record.id.clone(),
            file_name: entry.file_name.clone(),
            class_name: entry.record.class_name.clone(),
            class_hash: class_info
                .as_ref()
                .map(|class_info| class_info.hash.clone())
                .unwrap_or_default(),
            source_action: entry.record.source_action.clone(),
            live: !entry.record.is_nullified(),
            nullifier: entry.record.nullifier.clone(),
            grounded,
            fields: entry.record.fields_map(),
            predicate_source: class_info
                .map(|class_info| class_info.predicate_source)
                .unwrap_or_else(|| format!("Is{}(state) = OR(...)", entry.record.class_name)),
        }
    }

    fn class_summary(&self, class_info: CatalogClass) -> ClassSummary {
        ClassSummary {
            name: class_info.name,
            emoji: class_info.emoji,
            hash: class_info.hash,
            description: class_info.description,
            live_count: 0,
            produced_by: class_info.produced_by,
            consumed_by: class_info.consumed_by,
            predicate_source: class_info.predicate_source,
        }
    }
}

fn reconcile_objects(
    paths: &DriverPaths,
    objects: &mut [ObjectFileEntry],
    on_chain_nullifiers: &HashSet<Hash>,
) {
    for entry in objects.iter_mut() {
        if entry.record.is_nullified() {
            continue;
        }
        let nullifier_hash = match object_nullifier_hash(&entry.record.obj) {
            Ok(hash) => hash,
            Err(_) => continue,
        };
        if !on_chain_nullifiers.contains(&nullifier_hash) {
            continue;
        }
        let nullified_record = ObjectRecord {
            id: entry.record.id.clone(),
            class_name: entry.record.class_name.clone(),
            source_action: entry.record.source_action.clone(),
            nullifier: Some(encode_hash_hex(&nullifier_hash)),
            pod: entry.record.pod.clone(),
            obj: entry.record.obj.clone(),
            tx: entry.record.tx.clone(),
        };
        if let Err(err) = write_object_file(paths, &nullified_record, &entry.file_name) {
            eprintln!(
                "zk-craft: reconcile failed to nullify {}: {err}",
                entry.file_name
            );
            continue;
        }
        entry.record = nullified_record;
    }
}

fn validate_execute_request(input: &ExecuteActionInput, action: &ActionSummary) -> Result<()> {
    if input.input_objects.len() != action.input_classes.len() {
        return Err(anyhow!(
            "{} expects {} inputs, got {}",
            input.action_id,
            action.input_classes.len(),
            input.input_objects.len()
        ));
    }

    let mut seen = HashSet::new();
    for selector in &input.input_objects {
        let key = match selector {
            ObjectSelector::FileName(file_name) => format!("file:{file_name}"),
            ObjectSelector::ObjectId(object_id) => format!("id:{object_id}"),
        };
        if !seen.insert(key.clone()) {
            return Err(anyhow!("duplicate input object selector is not allowed: {key}"));
        }
    }

    Ok(())
}

#[derive(Debug, Clone)]
struct ResolvedInput {
    file_name: String,
    record: ObjectRecord,
}

fn resolve_inputs(
    entries: &[ObjectFileEntry],
    input: &ExecuteActionInput,
    action: &ActionSummary,
) -> Result<Vec<ResolvedInput>> {
    let mut resolved_inputs = Vec::new();
    for (slot, selector) in input.input_objects.iter().enumerate() {
        let expected_class = action.input_classes[slot].as_str();
        let entry = select_object(entries, selector)?;
        if entry.record.is_nullified() {
            return Err(anyhow!("input object is not live: {}", entry.record.id));
        }
        if entry.record.class_name != expected_class {
            return Err(anyhow!(
                "input class mismatch for {}: expected {}, got {}",
                entry.record.id,
                expected_class,
                entry.record.class_name
            ));
        }
        resolved_inputs.push(ResolvedInput {
            file_name: entry.file_name.clone(),
            record: entry.record.clone(),
        });
    }
    Ok(resolved_inputs)
}

#[derive(Debug)]
struct SavedFiles {
    output_files: Vec<String>,
    nullified_files: Vec<String>,
}

fn save_results(
    paths: &DriverPaths,
    action: &ActionSummary,
    action_id: &str,
    resolved_inputs: &[ResolvedInput],
    spendable_outputs: &SpendableObjects,
) -> Result<SavedFiles> {
    let mut nullified_files = Vec::new();
    for input in resolved_inputs {
        let input_record = &input.record;
        let spendable = input_record.spendable();
        let nullifier_hash = object_nullifier_hash(&spendable.obj).map_err(|err| {
            anyhow!(
                "failed to compute input nullifier for {}: {err}",
                input_record.id
            )
        })?;
        let input_nullifier = encode_hash_hex(&nullifier_hash);

        let nullified_record = ObjectRecord {
            id: input_record.id.clone(),
            class_name: input_record.class_name.clone(),
            source_action: input_record.source_action.clone(),
            nullifier: Some(input_nullifier),
            pod: input_record.pod.clone(),
            obj: input_record.obj.clone(),
            tx: input_record.tx.clone(),
        };
        write_object_file(paths, &nullified_record, &input.file_name)?;
        nullified_files.push(input.file_name.clone());
    }

    if spendable_outputs.objs.len() != action.output_classes.len() {
        return Err(anyhow!(
            "action {} output mismatch: descriptor expects {}, engine returned {}",
            action_id,
            action.output_classes.len(),
            spendable_outputs.objs.len()
        ));
    }

    let mut output_files = Vec::new();
    for (index, class_name) in action.output_classes.iter().enumerate() {
        let spendable = spendable_outputs.obj(index);
        let object_id = format!("{:#}", spendable.obj.commitment());
        let file_name = format!(
            "{}_{}.dobj",
            class_name.to_ascii_lowercase(),
            object_id.to_ascii_lowercase()
        );
        output_files.push(file_name.clone());

        let live_record = ObjectRecord {
            id: object_id,
            class_name: class_name.clone(),
            source_action: action_id.to_string(),
            nullifier: None,
            pod: spendable.pod,
            obj: spendable.obj,
            tx: spendable.tx,
        };
        write_object_file(paths, &live_record, &file_name)?;
    }

    Ok(SavedFiles {
        output_files,
        nullified_files,
    })
}

fn rollback_results(paths: &DriverPaths, resolved_inputs: &[ResolvedInput], saved: &SavedFiles) {
    for input in resolved_inputs {
        let live_record = ObjectRecord {
            id: input.record.id.clone(),
            class_name: input.record.class_name.clone(),
            source_action: input.record.source_action.clone(),
            nullifier: None,
            pod: input.record.pod.clone(),
            obj: input.record.obj.clone(),
            tx: input.record.tx.clone(),
        };
        if let Err(err) = write_object_file(paths, &live_record, &input.file_name) {
            eprintln!("zk-craft: rollback failed for {}: {err}", input.file_name);
        }
    }
    for file_name in &saved.output_files {
        let path = paths.objects_dir.join(file_name);
        match std::fs::remove_file(&path) {
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                eprintln!("zk-craft: rollback failed to remove {file_name}: {err}");
            }
        }
    }
}

fn build_relayer_payload(old_state_root_hash: &Hash, action_output: &SpendableObjects) -> Result<Vec<u8>> {
    let params = Params::default();
    let shrunk_main_pod = ShrunkMainPodSetup::new(&params)
        .build()
        .map_err(|err| anyhow!("failed to build shrunk proof circuit: {err}"))?;
    let compressed = shrink_compress_pod(&shrunk_main_pod, action_output.tx_pod.clone())
        .map_err(|err| anyhow!("failed to shrink/compress tx proof: {err}"))?;

    let tx_final = action_output.tx.dict().commitment();
    let nullifiers = action_output
        .tx
        .nullifiers
        .iter()
        .map(|entry| Ok(Hash(entry?.raw().0)))
        .collect::<Result<Vec<_>>>()?;
    let payload = Payload {
        proof: PayloadProof::Plonky2(Box::new(compressed)),
        tx_final,
        state_root_hash: *old_state_root_hash,
        nullifiers,
    };

    Ok(payload.to_bytes())
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;

    use anyhow::Result;
    use common::test_state::TestState;
    use craft_sdk::{Helper, SpendableObject, SpendableObjects};
    use tempfile::tempdir;
    use txlib::{GroundingWitness, StateRoot};

    use super::*;
    use crate::builtin::{actions, dependencies};
    use crate::catalog::ActionCatalog;
    use crate::clients::{RelayerConfirmation, SynchronizerHead, SynchronizerMembership};

    fn temp_paths() -> DriverPaths {
        let dir = tempdir().unwrap();
        let root = dir.keep();
        let settings_dir = root.join("config/com.dobjlabs.zk-craft");
        let settings_path = settings_dir.join("settings.json");
        let objects_dir = root.join(".objects");
        let nullified_objects_dir = objects_dir.join(".nullified");
        DriverPaths {
            settings_dir,
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
        let catalog = make_catalog();
        let outputs = catalog
            .execute_action("FindLog".to_string(), dummy_grounding_witness(), vec![])
            .unwrap();
        let spendable = outputs.obj(0);
        let id = format!("{:#}", spendable.obj.commitment());
        let record = ObjectRecord {
            id,
            class_name: "Log".to_string(),
            source_action: "FindLog".to_string(),
            nullifier: None,
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
        fn list_actions(&self) -> Vec<ActionSummary> {
            self.inner.list_actions()
        }

        fn get_action(&self, action_id: &str) -> Option<ActionSummary> {
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
        fail_wait: bool,
    }

    impl RelayerClient for MockRelayer {
        fn submit_proof(
            &self,
            _relayer_api_url: &str,
            _payload_bytes: &[u8],
            _client_ref: Option<String>,
        ) -> Result<relayer::api_types::SubmitProofResponse> {
            Ok(relayer::api_types::SubmitProofResponse {
                job_id: "job-1".to_string(),
                status: relayer::api_types::JobStatus::Queued,
                tx_final: "0x0".to_string(),
                state_root_hash: "0x0".to_string(),
                attempt_count: 0,
                created_at: 0,
            })
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
        assert!(filtered.iter().all(|action| action.input_classes.contains(&"Wood".to_string())));
    }

    #[test]
    fn test_execute_rolls_back_outputs_on_relayer_failure() {
        let (entry, mut deps) = make_input_record("log_1.dobj");
        deps.relayer = Arc::new(MockRelayer { fail_wait: true });
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

        let remaining = load_object_files(&paths).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].file_name, "log_1.dobj");
        assert!(!remaining[0].record.is_nullified());
    }
}
