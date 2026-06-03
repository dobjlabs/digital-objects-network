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
use crate::error::DriverError;
use crate::execute::{
    build_relayer_payload, obj_type_hash, reconcile_objects, resolve_inputs, save_results,
    update_output_files, validate_execute_request,
};
use crate::object_record::{ObjectRecord, parse_object_record_file};
use crate::object_store::{
    ObjectFileEntry, ensure_store_dirs, load_object_files, matches_query, write_object_file,
};
use crate::pexe_catalog::PexeCatalog;
use crate::settings::{default_settings, read_settings, write_settings};
use crate::types::{
    ActionQuery, DriverPaths, ExecuteActionInput, ExecuteActionResult, ExecutionReporter,
    ExecutionStepContext, NoopExecutionReporter, ObjectQuery,
};
use wire_types::{
    ActionSummary, CheckActionCandidate, CheckActionReport, ClassSummary, DriverSettings,
    ExecutionPhase, ObjectStatus, ObjectSummary, QualifiedName,
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
            .map(|entry| self.object_summary(entry))
            .collect())
    }

    /// Resolve an object by either a bare basename (`Wood.dobj`) or an
    /// absolute path. Only the file name is consulted, so user-pasted
    /// paths work the same as basenames, and `..` segments can't
    /// escape the inventory.
    ///
    /// Missing files produce [`DriverError::ObjectFileNotFound`] (HTTP
    /// 404), not a generic 500.
    pub fn read_object(&self, path: &Path) -> Result<ObjectSummary> {
        let file_name = extract_basename(path)?;
        let resolved = self.resolve_managed_path(&file_name);
        if !resolved.exists() {
            return Err(DriverError::ObjectFileNotFound(file_name).into());
        }
        let record = parse_object_record_file(&resolved)?;
        Ok(self.object_summary(&ObjectFileEntry { file_name, record }))
    }

    /// Look up a basename in the live dir, falling back to the nullified
    /// dir. Either path is returned even if the file doesn't exist on
    /// disk — the caller is expected to follow up with an existence check
    /// or a parse attempt.
    fn resolve_managed_path(&self, file_name: &str) -> std::path::PathBuf {
        let live = self.paths.objects_dir.join(file_name);
        if live.exists() {
            live
        } else {
            self.paths.nullified_objects_dir.join(file_name)
        }
    }

    pub fn sync_inventory(&self, query: Option<&ObjectQuery>) -> Result<Vec<ObjectSummary>> {
        let mut entries = load_object_files(&self.paths)?;
        let settings = self.load_settings()?;
        let object_commitments = entries
            .iter()
            .map(|entry| entry.record.obj.commitment())
            .collect::<HashSet<_>>();
        let all_nullifiers = entries
            .iter()
            .filter_map(|entry| object_nullifier_hash(&entry.record.obj).ok())
            .collect::<HashSet<_>>();

        let membership = self.deps.synchronizer.fetch_membership_with_nullifiers(
            &settings.synchronizer_api_url,
            &object_commitments.iter().copied().collect::<Vec<_>>(),
            &all_nullifiers.iter().copied().collect::<Vec<_>>(),
        )?;

        reconcile_objects(
            &self.paths,
            &mut entries,
            &membership.created_objects,
            &membership.on_chain_nullifiers,
        )?;

        // Resolve correct Ethereum tx hashes from the relayer for any
        // objects whose hash may be stale due to fee-bump replacements.
        for entry in entries.iter_mut() {
            if entry.record.status != ObjectStatus::Pending {
                continue;
            }
            let tx_final = encode_hash_hex(&entry.record.tx_final);
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
            .map(|entry| self.object_summary(entry))
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

    /// Look up a single action by its qualified name. Errors if no plugin
    /// provides the action.
    pub fn get_action(&self, action: &QualifiedName) -> Result<ActionSummary> {
        self.deps
            .catalog
            .get_action(action)
            .ok_or_else(|| anyhow!("unknown action: {action}"))
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

    /// Import an external `.dobj` object — one not produced by this driver
    /// (e.g. a file from outside `~/.dobj/`) — into local inventory.
    ///
    /// `dobj_json` is the raw JSON contents of a `.dobj` file. The object is
    /// filed under a canonical name derived from its commitment — the file's
    /// own `id` and filename are not trusted.
    ///
    /// Validation:
    /// - the `class` must be one this driver's catalog knows, and the object's
    ///   committed `obj["type"]` predicate hash must equal that class's hash
    ///   (the same belt-and-suspenders check `execute` runs on its inputs);
    /// - the object must not already be in inventory (live or nullified);
    /// - the object's nullifier must not already be spent on-chain.
    ///
    /// Status is decided by grounding: `Live` if the source tx is canonical,
    /// otherwise `Unknown` (a later `sync_inventory` promotes it). If the
    /// synchronizer is unreachable the object is imported as `Unknown` rather
    /// than failing, so the flow still works offline.
    pub fn import_object(&self, dobj_json: &str) -> Result<ObjectSummary> {
        let mut record: ObjectRecord = serde_json::from_str(dobj_json).map_err(|err| {
            DriverError::InvalidInput(format!("could not parse .dobj contents: {err}"))
        })?;

        // 1. Class must be known, and the pod's committed `type` hash must
        //    match the catalog's class hash. A mismatch on either is fatal —
        //    the second check catches a `class` label that drifted from the
        //    object's actual pod-level identity.
        let class_info = self
            .deps
            .catalog
            .get_class(&record.class)
            .ok_or_else(|| DriverError::UnknownClass(record.class.to_string()))?;
        let expected_class_hash = decode_hash_hex(class_info.hash.as_str()).map_err(|err| {
            anyhow!(
                "catalog class {} has an unreadable hash: {err}",
                record.class
            )
        })?;
        let actual_class_hash = obj_type_hash(&record.obj).ok_or_else(|| {
            DriverError::InvalidInput("imported object has no readable 'type' field".to_string())
        })?;
        if actual_class_hash != expected_class_hash {
            return Err(DriverError::InvalidInput(format!(
                "class hash mismatch: pod 'type' = {actual_class_hash:#}, class {} expects {}",
                record.class, class_info.hash
            ))
            .into());
        }

        // 2. Recompute id + file name from the commitment; never trust the
        //    sender's. The id is self-certifying — it IS the commitment.
        let object_id = format!("{:#}", record.obj.commitment());
        let file_name = format!(
            "{}_{}.{}",
            record.class.file_prefix(),
            object_id.to_ascii_lowercase(),
            crate::paths::DOBJ_EXTENSION
        );
        record.id = object_id.clone();

        // 3. Reject if we already hold this object (live or nullified).
        let entries = load_object_files(&self.paths)?;
        if entries
            .iter()
            .any(|entry| entry.record.id == object_id || entry.file_name == file_name)
        {
            return Err(
                DriverError::Conflict(format!("object already in inventory: {file_name}")).into(),
            );
        }

        // 4. Grounding decides status. A nullifier already on-chain means the
        //    object has been spent — reject it. Otherwise Live if the object's
        //    commitment is in the canonical created set, else Unknown. Tolerate
        //    an unreachable synchronizer by importing as Unknown.
        let settings = self.load_settings()?;
        let commitment = record.obj.commitment();
        let nullifier = object_nullifier_hash(&record.obj).map_err(|err| {
            DriverError::InvalidInput(format!(
                "could not derive nullifier from imported object: {err}"
            ))
        })?;
        let status = match self.deps.synchronizer.fetch_membership_with_nullifiers(
            &settings.synchronizer_api_url,
            &[commitment],
            &[nullifier],
        ) {
            Ok(membership) => {
                if membership.on_chain_nullifiers.contains(&nullifier) {
                    return Err(DriverError::Conflict(
                        "imported object has already been spent on-chain".to_string(),
                    )
                    .into());
                } else if membership.created_objects.contains(&commitment) {
                    ObjectStatus::Live
                } else {
                    ObjectStatus::Unknown
                }
            }
            Err(err) => {
                log::warn!(
                    "import: grounding check failed, importing {file_name} as unknown: {err:#}"
                );
                ObjectStatus::Unknown
            }
        };
        record.status = status;

        // 5. File it. `write_object_file` routes to the live or nullified dir
        //    based on status, so a freshly-imported Unknown/Live lands live.
        write_object_file(&self.paths, &record, &file_name)?;

        Ok(self.object_summary(&ObjectFileEntry { file_name, record }))
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
        reporter.on_step(ExecutionPhase::GenerateProof, "Verifying inputs", &no_ctx);
        let entries = load_object_files(&self.paths)?;
        let resolved_inputs = resolve_inputs(&entries, &input, &action)?;
        let input_commitments = resolved_inputs
            .iter()
            .map(|entry| entry.record.obj.commitment())
            .collect::<Vec<_>>();
        let grounding_witness = self
            .deps
            .synchronizer
            .fetch_grounding_witness(&settings.synchronizer_api_url, &input_commitments)?;
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

        let mut commit_ctx = ExecutionStepContext {
            old_root: Some(old_root.clone()),
            output_files: Vec::new(),
            output_status: None,
        };
        reporter.on_step(ExecutionPhase::Commit, "Shrinking proof", &commit_ctx);
        let payload_bytes = self
            .deps
            .payload_builder
            .build_payload(&old_root_hash, &spendable_outputs)?;
        // A tx has landed once the union of its effects is canonical: every
        // produced object commitment in the created set and every nullifier in
        // the nullifier set. Collect both for the confirmation poll below.
        let output_commitments: Vec<Hash> = spendable_outputs
            .objs
            .iter()
            .map(|spendable| spendable.obj.commitment())
            .collect();
        let nullifiers = spendable_outputs.tx.nullifier_hashes()?;

        reporter.on_step(ExecutionPhase::Commit, "Creating files", &commit_ctx);
        let saved = save_results(&self.paths, &action, &resolved_inputs, &spendable_outputs)?;
        commit_ctx = file_write_ctx(&old_root, &saved.output_files, ObjectStatus::Unknown);
        reporter.on_step(
            ExecutionPhase::Commit,
            "Output object files created with status unknown",
            &commit_ctx,
        );

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
            Ok(resp) if resp.status == wire_types::relayer::JobStatus::Failed => {
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
        commit_ctx = file_write_ctx(&old_root, &saved.output_files, ObjectStatus::Pending);
        reporter.on_step(
            ExecutionPhase::Commit,
            "Got transaction hash; output object files updated to pending while waiting for submission confirmation",
            &commit_ctx,
        );

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
            reporter.on_step(
                ExecutionPhase::Commit,
                "Got replacement transaction hash; output object files updated to pending",
                &commit_ctx,
            );
        }

        reporter.on_step(
            ExecutionPhase::Commit,
            "Waiting for synchronizer to observe commit",
            &commit_ctx,
        );
        let sync_head = match self.deps.synchronizer.wait_for_tx(
            &settings.synchronizer_api_url,
            &output_commitments,
            &nullifiers,
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
                commit_ctx = file_write_ctx(&old_root, &saved.output_files, ObjectStatus::Unknown);
                reporter.on_step(
                    ExecutionPhase::Commit,
                    "Synchronizer did not observe commit; output object files reverted to unknown",
                    &commit_ctx,
                );
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
        commit_ctx = file_write_ctx(&old_root, &saved.output_files, ObjectStatus::Live);
        reporter.on_step(
            ExecutionPhase::Commit,
            "Commit observed; output object files updated to live",
            &commit_ctx,
        );

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

    fn object_summary(&self, entry: &ObjectFileEntry) -> ObjectSummary {
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

fn file_write_ctx(
    old_root: &str,
    output_files: &[String],
    output_status: ObjectStatus,
) -> ExecutionStepContext {
    ExecutionStepContext {
        old_root: Some(old_root.to_string()),
        output_files: output_files.to_vec(),
        output_status: Some(output_status),
    }
}

/// Normalize an input path (absolute or basename) to its file name. Used
/// by `read_object` and by action execution to turn user-supplied paths
/// into managed-store basenames before lookup.
pub(crate) fn extract_basename(path: &Path) -> Result<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|s| s.to_string())
        .ok_or_else(|| {
            DriverError::InvalidInput(format!("missing file name in path: {}", path.display()))
                .into()
        })
}
