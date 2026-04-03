use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use common::encode_hash_hex;
use craft_sdk::SpendableObjects;
use pod2::middleware::Hash;
use txlib::object_nullifier_hash;

use crate::builtin::BuiltinActionCatalog;
use crate::catalog::{ActionCatalog, CatalogClass};
use crate::clients::{
    HttpRelayerClient, HttpSynchronizerClient, RELAYER_POLL_INTERVAL_MS,
    RELAYER_POLL_TIMEOUT_SECS, RelayerClient, SYNCHRONIZER_POLL_INTERVAL_MS,
    SYNCHRONIZER_POLL_TIMEOUT_SECS, SynchronizerClient,
};
use crate::execute::{
    build_relayer_payload, reconcile_objects, resolve_inputs, rollback_results, save_results,
    validate_execute_request,
};
use crate::object_store::{
    ObjectFileEntry, ensure_store_dirs, load_object_files, matches_query, parse_object_file_from_path,
    select_object,
};
use crate::runtime::ActionRunGate;
use crate::settings::{default_settings, read_settings, write_settings};
use crate::types::{
    ActionQuery, ActionSummary, CheckActionCandidate, CheckActionReport, ClassSummary,
    DriverPaths, DriverSettings, ExecuteActionInput, ExecuteActionResult, ExecutionPhase,
    ExecutionReporter, NoopExecutionReporter, ObjectDetail, ObjectQuery, ObjectSelector,
    ObjectSummary,
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
        Self::open(crate::paths::default_paths()?, DriverDeps::default())
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
            if let Some(entry) = live_objects.iter().find(|entry| {
                &entry.record.class_name == required_class && !used_ids.contains(&entry.record.id)
            }) {
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
        let saved = save_results(
            &self.paths,
            &action,
            &input.action_id,
            &resolved_inputs,
            &spendable_outputs,
        )?;

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
