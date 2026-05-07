use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use common::{decode_hash_hex, encode_hash_hex};
use pod2::middleware::Hash;
use sdk::SpendableObjects;
use txlib::object_nullifier_hash;

use crate::catalog::{ActionCatalog, CatalogClass};
use crate::clients::{
    HttpRelayerClient, HttpSynchronizerClient, RELAYER_POLL_INTERVAL_MS, RELAYER_POLL_TIMEOUT_SECS,
    RelayerClient, SYNCHRONIZER_POLL_INTERVAL_MS, SYNCHRONIZER_POLL_TIMEOUT_SECS,
    SynchronizerClient,
};
use crate::execute::{
    build_relayer_payload, obj_type_hash, reconcile_objects, resolve_inputs, save_results,
    update_output_files, validate_execute_request,
};
use crate::object_record::ObjectStatus;
use crate::object_record::parse_object_record_file;
use crate::object_store::{
    ObjectFileEntry, ensure_store_dirs, load_object_files, matches_query, select_object,
    write_object_file,
};
use crate::pexe_catalog::PexeCatalog;
use crate::qualified_name::QualifiedName;
use crate::settings::{default_settings, read_settings, write_settings};
use crate::types::{
    ActionQuery, ActionSummary, CheckActionCandidate, CheckActionReport, ClassSummary, DriverPaths,
    DriverSettings, ExecuteActionInput, ExecuteActionResult, ExecutionPhase, ExecutionReporter,
    ExecutionStepContext, NoopExecutionReporter, ObjectQuery, ObjectSelector, ObjectSummary,
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

impl DriverDeps {
    /// Build deps with a catalog loaded from `paths.actions_dir`.
    pub fn load(paths: &DriverPaths) -> Result<Self> {
        let catalog = PexeCatalog::load(&paths.actions_dir)?;
        if catalog.plugin_count() == 0 {
            log::warn!(
                "no .pexe plugins installed in {}; run `just install-plugins`",
                paths.actions_dir.display()
            );
        }
        Ok(Self {
            catalog: Arc::new(catalog),
            synchronizer: Arc::new(HttpSynchronizerClient),
            relayer: Arc::new(HttpRelayerClient),
            payload_builder: Arc::new(DefaultPayloadBuilder),
        })
    }
}

#[derive(Clone)]
pub struct Driver {
    paths: DriverPaths,
    deps: DriverDeps,
}

impl Driver {
    pub fn open_default() -> Result<Self> {
        let paths = crate::paths::default_paths()?;
        ensure_store_dirs(&paths)?;
        let deps = DriverDeps::load(&paths)?;
        Ok(Self { paths, deps })
    }

    pub fn open(paths: DriverPaths, deps: DriverDeps) -> Result<Self> {
        ensure_store_dirs(&paths)?;
        Ok(Self { paths, deps })
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

    pub fn read_object(&self, selector: &ObjectSelector) -> Result<ObjectSummary> {
        let entries = load_object_files(&self.paths)?;
        let entry = select_object(&entries, selector)?;
        Ok(self.object_summary(entry, None))
    }

    pub fn read_object_file(&self, path: &Path) -> Result<ObjectSummary> {
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| anyhow!("invalid input path (missing file name): {}", path.display()))?
            .to_string();
        let record = parse_object_record_file(path)?;
        Ok(self.object_summary(&ObjectFileEntry { file_name, record }, None))
    }

    pub fn sync_inventory(&self, query: Option<&ObjectQuery>) -> Result<Vec<ObjectSummary>> {
        let mut entries = load_object_files(&self.paths)?;
        let settings = self.load_settings()?;
        let source_tx_hashes = entries
            .iter()
            .map(|entry| entry.record.spendable().tx.dict().commitment())
            .collect::<HashSet<_>>();
        let all_nullifiers = entries
            .iter()
            .filter_map(|entry| object_nullifier_hash(&entry.record.obj).ok())
            .collect::<HashSet<_>>();

        let membership = self.deps.synchronizer.fetch_membership_with_nullifiers(
            &settings.synchronizer_api_url,
            &source_tx_hashes.iter().copied().collect::<Vec<_>>(),
            &all_nullifiers.iter().copied().collect::<Vec<_>>(),
        )?;

        reconcile_objects(
            &self.paths,
            &mut entries,
            &membership.grounded_txs,
            &membership.on_chain_nullifiers,
        )?;

        // Resolve correct Ethereum tx hashes from the relayer for any
        // objects whose hash may be stale due to fee-bump replacements.
        for entry in entries.iter_mut() {
            if entry.record.status != ObjectStatus::Pending {
                continue;
            }
            let tx_final = encode_hash_hex(&entry.record.tx.dict().commitment());
            let current_hash = self
                .deps
                .relayer
                .lookup_tx_hash(&settings.relayer_api_url, &tx_final);
            if let Ok(Some(relayer_hash)) = current_hash
                && entry.record.tx_hash.as_deref() != Some(&relayer_hash)
            {
                entry.record.tx_hash = Some(relayer_hash);
                let _ = write_object_file(&self.paths, &entry.record, &entry.file_name);
            }
        }

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
                    query.action.as_ref().is_none_or(|q| &action.action == q)
                        && query
                            .input_class
                            .as_ref()
                            .is_none_or(|c| action.total_inputs.iter().any(|r| r.class == *c))
                        && query
                            .output_class
                            .as_ref()
                            .is_none_or(|c| action.total_outputs.iter().any(|r| r.class == *c))
                })
            })
            .collect())
    }

    pub fn list_classes(&self) -> Result<Vec<ClassSummary>> {
        let live_objects = load_object_files(&self.paths)?
            .into_iter()
            .filter(|entry| entry.record.status == ObjectStatus::Live)
            .collect::<Vec<_>>();
        Ok(self
            .deps
            .catalog
            .list_classes()
            .into_iter()
            .map(|class_info| ClassSummary {
                live_count: live_objects
                    .iter()
                    .filter(|entry| entry.record.class == class_info.class)
                    .count(),
                ..self.class_summary(class_info)
            })
            .collect())
    }

    pub fn get_class(&self, class: &QualifiedName) -> Result<ClassSummary> {
        let class_info = self
            .deps
            .catalog
            .get_class(class)
            .ok_or_else(|| anyhow!("unknown class: {class}"))?;
        let live_count = load_object_files(&self.paths)?
            .into_iter()
            .filter(|entry| {
                entry.record.status == ObjectStatus::Live && entry.record.class == *class
            })
            .count();
        Ok(ClassSummary {
            live_count,
            ..self.class_summary(class_info)
        })
    }

    pub fn check_action(&self, action: &QualifiedName) -> Result<CheckActionReport> {
        let action_summary = self
            .deps
            .catalog
            .get_action(action)
            .ok_or_else(|| anyhow!("unknown action: {action}"))?;
        let entries = load_object_files(&self.paths)?;
        let live_objects = entries
            .iter()
            .filter(|entry| entry.record.status == ObjectStatus::Live)
            .collect::<Vec<_>>();

        let mut available = Vec::new();
        let mut missing_inputs = Vec::new();
        let mut used_ids = HashSet::new();

        // Apply the same cryptographic check that `resolve_inputs` runs at
        // execute time: a candidate must match by qualified class AND its
        // on-chain `obj["type"]` predicate hash must equal the action's
        // required class hash. Without the hash check, a tampered or
        // stale-migration .dobj would be reported as feasible here and then
        // rejected at execute, wasting proof-generation time.
        for required in &action_summary.total_inputs {
            let expected_hash = decode_hash_hex(required.hash.as_str()).ok();
            let candidate = live_objects.iter().find(|entry| {
                entry.record.class == required.class
                    && !used_ids.contains(&entry.record.id)
                    && matches!(
                        (expected_hash, obj_type_hash(&entry.record.obj)),
                        (Some(expected), Some(actual)) if expected == actual
                    )
            });
            if let Some(entry) = candidate {
                used_ids.insert(entry.record.id.clone());
                available.push(CheckActionCandidate {
                    class: required.class.clone(),
                    object_id: entry.record.id.clone(),
                    file_name: entry.file_name.clone(),
                });
            } else {
                missing_inputs.push(required.clone());
            }
        }

        Ok(CheckActionReport {
            feasible: missing_inputs.is_empty(),
            action: action.clone(),
            available_inputs: available,
            missing_inputs,
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
        let settings = self.load_settings()?;
        let action = self
            .deps
            .catalog
            .get_action(&input.action)
            .ok_or_else(|| anyhow!("unknown action: {}", input.action))?;

        validate_execute_request(&input, &action)?;

        let no_ctx = ExecutionStepContext::default();
        reporter.on_step(ExecutionPhase::GenerateProof, "Verifying Inputs", &no_ctx);
        let entries = load_object_files(&self.paths)?;
        let resolved_inputs = resolve_inputs(&entries, &input, &action)?;
        let source_tx_hashes = resolved_inputs
            .iter()
            .map(|entry| entry.record.spendable().tx.dict().commitment())
            .collect::<Vec<_>>();
        let grounding_witness = self
            .deps
            .synchronizer
            .fetch_grounding_witness(&settings.synchronizer_api_url, &source_tx_hashes)?;
        let old_root_hash = grounding_witness.state_root.hash();
        let old_root = encode_hash_hex(&old_root_hash);

        reporter.on_step(ExecutionPhase::GenerateProof, "Generating proof", &no_ctx);
        let execution_inputs = resolved_inputs
            .iter()
            .map(|input| input.record.spendable())
            .collect::<Vec<_>>();
        let spendable_outputs = self.deps.catalog.execute_action(
            input.action.clone(),
            grounding_witness,
            execution_inputs,
        )?;
        reporter.on_done(ExecutionPhase::GenerateProof, None);

        let commit_ctx = ExecutionStepContext {
            old_root: Some(old_root.clone()),
        };
        reporter.on_step(ExecutionPhase::Commit, "Shrinking proof", &commit_ctx);
        let payload_bytes = self
            .deps
            .payload_builder
            .build_payload(&old_root_hash, &spendable_outputs)?;
        let expected_tx_final = spendable_outputs.tx.dict().commitment();

        reporter.on_step(ExecutionPhase::Commit, "Creating files", &commit_ctx);
        let saved = save_results(&self.paths, &action, &resolved_inputs, &spendable_outputs)?;

        // Submit to relayer. Output files are kept as Unknown on failure so
        // the user can retry submission later without regenerating proofs.
        reporter.on_step(
            ExecutionPhase::Commit,
            "Submitting proof to relayer",
            &commit_ctx,
        );
        let submit_response = match self.deps.relayer.submit_proof(
            &settings.relayer_api_url,
            &payload_bytes,
            Some(format!("driver:{}", input.action)),
        ) {
            Ok(resp) if resp.status == relayer::api_types::JobStatus::Failed => {
                return Err(anyhow!("relayer rejected job {} immediately", resp.job_id));
            }
            Ok(resp) => resp,
            Err(err) => {
                return Err(err);
            }
        };

        // Past this point the proof has been accepted by the relayer and may
        // land on-chain at any moment. Output files stay as Unknown until the
        // relayer broadcasts and we have a tx_hash.
        let waiting_label = format!("Waiting for relayer job {}", submit_response.job_id);
        reporter.on_step(ExecutionPhase::Commit, &waiting_label, &commit_ctx);

        // Poll until the relayer has broadcast the tx and we have a tx_hash,
        // then mark output files as Pending.
        let eth_tx_hash = self.deps.relayer.wait_for_tx_hash(
            &settings.relayer_api_url,
            &submit_response.job_id,
            RELAYER_POLL_TIMEOUT_SECS,
            RELAYER_POLL_INTERVAL_MS,
        )?;
        update_output_files(
            &self.paths,
            &saved.output_files,
            ObjectStatus::Pending,
            Some(&eth_tx_hash),
        )?;

        let confirmation = self.deps.relayer.wait_for_confirmation(
            &settings.relayer_api_url,
            &submit_response.job_id,
            RELAYER_POLL_TIMEOUT_SECS,
            RELAYER_POLL_INTERVAL_MS,
        )?;

        // Use the confirmed tx_hash — it may differ from the initial one if
        // the relayer performed a fee-bump replacement while waiting.
        let final_tx_hash = confirmation.tx_hash.as_deref().unwrap_or(&eth_tx_hash);

        if final_tx_hash != eth_tx_hash {
            // Fee bump replaced the original tx; update .dobj files with the
            // new hash so they point at the on-chain transaction.
            update_output_files(
                &self.paths,
                &saved.output_files,
                ObjectStatus::Pending,
                Some(final_tx_hash),
            )?;
        }

        reporter.on_step(
            ExecutionPhase::Commit,
            "Waiting for synchronizer to observe commit",
            &commit_ctx,
        );
        let sync_head = match self.deps.synchronizer.wait_for_tx(
            &settings.synchronizer_api_url,
            expected_tx_final,
            SYNCHRONIZER_POLL_TIMEOUT_SECS,
            SYNCHRONIZER_POLL_INTERVAL_MS,
        ) {
            Ok(head) => head,
            Err(err) => {
                // Sync failed or timed out — revert outputs to Unknown but
                // keep the txHash so the chain submission can be inspected.
                // The next sync_inventory will reconcile if the tx lands later.
                update_output_files(
                    &self.paths,
                    &saved.output_files,
                    ObjectStatus::Unknown,
                    Some(final_tx_hash),
                )?;
                return Err(err);
            }
        };

        // Sync confirmed the tx — update output files to Live.
        update_output_files(
            &self.paths,
            &saved.output_files,
            ObjectStatus::Live,
            Some(final_tx_hash),
        )?;

        let result = ExecuteActionResult {
            old_root,
            new_root: encode_hash_hex(&sync_head.current_gsr),
            output_files: saved.output_files.clone(),
            nullified_files: saved.nullified_files.clone(),
            relayer_job_id: confirmation.job_id,
            tx_hash: Some(final_tx_hash.to_string()),
            block_number: confirmation.block_number,
        };
        reporter.on_done(ExecutionPhase::Commit, Some(&result));
        Ok(result)
    }

    pub fn generated_podlang(&self) -> Option<String> {
        self.deps.catalog.generated_podlang()
    }

    fn object_summary(&self, entry: &ObjectFileEntry, grounded: Option<bool>) -> ObjectSummary {
        let class_hash = self
            .deps
            .catalog
            .get_class(&entry.record.class)
            .map(|c| c.hash)
            .unwrap_or_default();
        ObjectSummary {
            id: entry.record.id.clone(),
            file_name: entry.file_name.clone(),
            class: entry.record.class.clone(),
            class_hash,
            status: entry.record.status,
            tx_hash: entry.record.tx_hash.clone(),
            grounded,
            fields: entry.record.fields_map(),
        }
    }

    fn class_summary(&self, class_info: CatalogClass) -> ClassSummary {
        ClassSummary {
            class: class_info.class,
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
