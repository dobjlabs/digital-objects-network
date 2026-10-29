use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock, Weak};

use anyhow::{Context, Result, anyhow};
use payload::{decode_hash_hex, encode_hash_hex};
use pod2::middleware::Hash;
use sdk::SpendableObjects;
use txlib::object_nullifier_hash;
use wire_types::transactions::{TransactionRecord, TransactionState};

use crate::catalog::{ActionCatalog, CatalogClass};
use crate::clients::{
    HttpRelayerClient, HttpSynchronizerClient, RelayerClient, SynchronizerClient,
};
use crate::error::DriverError;
use crate::execute::{build_relayer_payload, obj_type_hash};
use crate::holds::HoldTable;
use crate::object_record::{FileStatus, ObjectRecord, parse_object_record_file};
use crate::object_store::{
    ObjectFileEntry, custom_file_name, ensure_store_dirs, imported_file_name, key_fingerprint,
    load_imported_keys, load_object_files, matches_query, object_file_name, record_imported_key,
    rename_live_object_file, store_has_file, write_object_file,
};
use crate::pexe_catalog::PexeCatalog;
use crate::pipeline::RecordIndex;
use crate::settings::{default_settings, read_settings, write_settings};
use crate::transactions::load_records;
use crate::types::{ActionQuery, DriverPaths, ObjectQuery};
use wire_types::{
    ActionSummary, CheckActionCandidate, CheckActionReport, ClassSummary, DriverSettings,
    ObjectStatus, ObjectSummary, QualifiedName,
};

pub trait PayloadBuilder: Send + Sync {
    fn build_payload(
        &self,
        old_state_root: &Hash,
        action_output: &SpendableObjects,
    ) -> Result<Vec<u8>>;
}

#[derive(Clone, Default)]
struct DefaultPayloadBuilder;

impl PayloadBuilder for DefaultPayloadBuilder {
    fn build_payload(
        &self,
        old_state_root: &Hash,
        action_output: &SpendableObjects,
    ) -> Result<Vec<u8>> {
        build_relayer_payload(old_state_root, action_output)
    }
}

#[derive(Clone)]
pub struct DriverDeps {
    pub catalog: Arc<RwLock<Arc<dyn ActionCatalog>>>,
    pub synchronizer: Arc<dyn SynchronizerClient>,
    pub relayer: Arc<dyn RelayerClient>,
    pub payload_builder: Arc<dyn PayloadBuilder>,
}

impl DriverDeps {
    /// Build deps with a catalog loaded from `paths.actions_dir`.
    pub fn load(paths: &DriverPaths) -> Result<Self> {
        let start = std::time::Instant::now();
        log::info!(
            "loading action catalog from {}...",
            paths.actions_dir.display()
        );
        let catalog = PexeCatalog::load(&paths.actions_dir)?;
        log::info!("action catalog loaded in {:?}", start.elapsed());
        if catalog.plugin_count() == 0 {
            log::warn!(
                "no .pexe plugins installed in {}; run `just install-plugins`",
                paths.actions_dir.display()
            );
        }
        Ok(Self {
            catalog: Arc::new(RwLock::new(Arc::new(catalog))),
            synchronizer: Arc::new(HttpSynchronizerClient::new()),
            relayer: Arc::new(HttpRelayerClient::new()),
            payload_builder: Arc::new(DefaultPayloadBuilder),
        })
    }
}

#[derive(Clone)]
pub struct Driver {
    pub(crate) paths: DriverPaths,
    pub(crate) deps: DriverDeps,
    /// Protects store read-check-write operations. Never held during proof
    /// generation or network requests.
    pub(crate) store_lock: Arc<Mutex<()>>,
    pub(crate) holds: HoldTable,
    pub(crate) reconciled_transactions: Arc<Mutex<HashMap<String, TransactionRecord>>>,
    pub(crate) submission_locks: Arc<Mutex<HashMap<Hash, Weak<Mutex<()>>>>>,
}

impl Driver {
    pub fn open_default() -> Result<Self> {
        let paths = crate::paths::default_paths()?;
        let deps = DriverDeps::load(&paths)?;
        Self::open(paths, deps)
    }

    pub fn open(paths: DriverPaths, deps: DriverDeps) -> Result<Self> {
        ensure_store_dirs(&paths)?;
        Ok(Self {
            paths,
            deps,
            store_lock: Arc::new(Mutex::new(())),
            holds: HoldTable::default(),
            reconciled_transactions: Arc::default(),
            submission_locks: Arc::default(),
        })
    }

    pub(crate) fn submission_lock(&self, tx_final: &Hash) -> Arc<Mutex<()>> {
        let mut locks = self
            .submission_locks
            .lock()
            .expect("submission lock table poisoned");
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks.get(tx_final).and_then(Weak::upgrade) {
            return lock;
        }
        let lock = Arc::new(Mutex::new(()));
        locks.insert(*tx_final, Arc::downgrade(&lock));
        lock
    }

    pub fn paths(&self) -> &DriverPaths {
        &self.paths
    }

    pub(crate) fn catalog(&self) -> Arc<dyn ActionCatalog> {
        self.deps
            .catalog
            .read()
            .expect("catalog lock poisoned")
            .clone()
    }

    /// Rebuild the action catalog from `actions_dir` and swap it in. Loads
    /// first, so a failed load (e.g. a bad .pexe) leaves the current catalog
    /// intact. Returns the new plugin count.
    pub fn reload_catalog(&self) -> Result<usize> {
        let catalog = PexeCatalog::load(&self.paths.actions_dir)?;
        let count = catalog.plugin_count();
        *self.deps.catalog.write().expect("catalog lock poisoned") = Arc::new(catalog);
        Ok(count)
    }

    /// Install a `.pexe` into the actions dir and hot-reload the catalog.
    /// Validates the archive and plugin name first. If the reload fails, the
    /// previously-installed plugin of the same name is restored (or the new
    /// file removed if there was none), so a failed upgrade can't destroy a
    /// working plugin or break the next startup. Returns the installed name.
    pub fn install_plugin(&self, bytes: &[u8]) -> Result<String> {
        let (manifest, _script) = pexe::unpack(bytes).context("invalid .pexe archive")?;
        let name = manifest.plugin.name.clone();
        crate::pexe_catalog::validate_plugin_name(&name)?;

        let dest = self
            .paths
            .actions_dir
            .join(format!("{name}.{}", pexe::PEXE_EXTENSION));
        // Move any same-named plugin aside first, so a failed reload can restore
        // it rather than leaving the user with nothing. The `.bak` name stays
        // out of the catalog scan, which only picks up `.pexe`.
        let backup = dest.exists().then(|| {
            self.paths
                .actions_dir
                .join(format!("{name}.{}.bak", pexe::PEXE_EXTENSION))
        });
        if let Some(backup) = &backup {
            std::fs::rename(&dest, backup)
                .with_context(|| format!("failed to back up {}", dest.display()))?;
        }

        // reload only swaps the in-memory catalog on success, so on failure the
        // running catalog still reflects the prior plugin - restoring the file
        // keeps disk and memory consistent.
        let outcome = pexe::install(bytes, &self.paths.actions_dir, &name)
            .and_then(|_| self.reload_catalog());
        match outcome {
            Ok(_) => {
                if let Some(backup) = backup {
                    let _ = std::fs::remove_file(backup);
                }
                Ok(name)
            }
            Err(err) => {
                let _ = std::fs::remove_file(&dest);
                if let Some(backup) = backup {
                    let _ = std::fs::rename(&backup, &dest);
                }
                Err(err.context(format!(
                    "plugin {name} failed to load; previous version left in place"
                )))
            }
        }
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
        let records = load_records(&self.paths)?;
        Ok(self
            .summaries(&entries, &records)
            .into_iter()
            .filter(|summary| query.is_none_or(|query| matches_query(summary, query)))
            .collect())
    }

    /// Resolve an object by either a bare basename (`Wood.dobj`) or an
    /// absolute path. Only the file name is consulted, so user-pasted
    /// paths work the same as basenames, and `..` segments can't
    /// escape the object store.
    ///
    /// Missing files produce [`DriverError::ObjectFileNotFound`] (HTTP
    /// 404), not a generic 500.
    pub fn read_object(&self, path: &Path) -> Result<ObjectSummary> {
        self.summary_of(self.read_object_entry(path)?)
    }

    /// Read summaries with shared transaction, hold, and catalog snapshots.
    /// Log and skip unreadable files; propagate transaction-directory errors.
    pub fn read_available_objects(&self, paths: &[impl AsRef<Path>]) -> Result<Vec<ObjectSummary>> {
        let records = load_records(&self.paths)?;
        let entries = paths
            .iter()
            .filter_map(|path| {
                self.read_object_entry(path.as_ref())
                    .inspect_err(|err| {
                        log::debug!("no summary for {}: {err:#}", path.as_ref().display())
                    })
                    .ok()
            })
            .collect::<Vec<_>>();
        Ok(self.summaries(&entries, &records))
    }

    fn read_object_entry(&self, path: &Path) -> Result<ObjectFileEntry> {
        let file_name = extract_basename(path)?;
        let resolved = self.resolve_managed_path(&file_name);
        if !resolved.exists() {
            return Err(DriverError::ObjectFileNotFound(file_name).into());
        }
        let record = parse_object_record_file(&resolved)?;
        Ok(ObjectFileEntry { file_name, record })
    }

    /// The object file's contents exactly as stored on disk, resolved the
    /// same way as [`Self::read_object`].
    pub fn read_object_raw(&self, path: &Path) -> Result<String> {
        let file_name = extract_basename(path)?;
        let resolved = self.resolve_managed_path(&file_name);
        if !resolved.exists() {
            return Err(DriverError::ObjectFileNotFound(file_name).into());
        }
        fs::read_to_string(&resolved)
            .with_context(|| format!("failed to read object file {file_name}"))
    }

    /// Moves an unspent object's file into the discarded directory, whatever
    /// its status, so the driver stops tracking it, and returns its summary
    /// from before the move. Spent objects are not found.
    ///
    /// None of the driver's usual guarantees about object state hold here:
    /// nothing is checked against the chain or reconciled, so a discarded
    /// object whose transaction has landed, or later lands, is simply no
    /// longer held, and the driver never reads the discarded directory back.
    pub fn discard_object_unsafe(&self, path: &Path) -> Result<ObjectSummary> {
        let file_name = extract_basename(path)?;
        let source = self.paths.objects_dir.join(&file_name);
        if !source.exists() {
            return Err(DriverError::ObjectFileNotFound(file_name).into());
        }
        let record = parse_object_record_file(&source)?;
        let summary = self.summary_of(ObjectFileEntry { file_name, record })?;
        fs::create_dir_all(&self.paths.discarded_objects_dir).with_context(|| {
            format!(
                "failed to create discarded directory {}",
                self.paths.discarded_objects_dir.display()
            )
        })?;
        fs::rename(
            &source,
            self.paths.discarded_objects_dir.join(&summary.file_name),
        )
        .with_context(|| format!("failed to discard object file {}", summary.file_name))?;
        Ok(summary)
    }

    /// Renames an unspent object's file to `new_name`, with `.dobj` added
    /// when missing, and returns its summary under the new name. Refuses a
    /// name that is unsafe or that another file has, and an object a proof
    /// holds or an open transaction spends or has yet to create.
    pub fn rename_object(&self, path: &Path, new_name: &str) -> Result<ObjectSummary> {
        let file_name = extract_basename(path)?;
        let _store = self.store_lock.lock().expect("store lock poisoned");
        let source = self.paths.objects_dir.join(&file_name);
        if !source.try_exists()? {
            return Err(DriverError::ObjectFileNotFound(file_name).into());
        }
        let record = parse_object_record_file(&source)?;
        let generated = object_file_name(&record.class, &record.obj)?;
        let target = custom_file_name(new_name, &generated)
            .map_err(|err| DriverError::InvalidInput(format!("{err:#}")))?;
        if target != file_name {
            if let Some(owner) = self.holds.labels().get(&file_name) {
                return Err(
                    DriverError::Conflict(format!("{file_name} is held by {owner}")).into(),
                );
            }
            for open in load_records(&self.paths)?.iter().filter(|r| r.is_open()) {
                if open.input_files.contains(&file_name) {
                    return Err(DriverError::Conflict(format!(
                        "{file_name} is spent by a transaction awaiting the chain"
                    ))
                    .into());
                }
                if open.output_files.contains(&file_name) {
                    return Err(DriverError::Conflict(format!(
                        "{file_name} is created by a transaction awaiting the chain"
                    ))
                    .into());
                }
            }
            // On a case-insensitive filesystem a change of case finds the
            // file itself under the new name.
            let taken = if target.eq_ignore_ascii_case(&file_name) {
                fs::read_dir(&self.paths.objects_dir)?
                    .filter_map(|entry| entry.ok())
                    .any(|entry| entry.file_name().to_str() == Some(target.as_str()))
            } else {
                store_has_file(&self.paths, &target)
            };
            if taken {
                return Err(DriverError::Conflict(format!("{target} already exists")).into());
            }
            rename_live_object_file(&self.paths, &file_name, &target)?;
        }
        self.summary_of(ObjectFileEntry {
            file_name: target,
            record,
        })
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

    pub fn list_actions(&self, query: Option<&ActionQuery>) -> Result<Vec<ActionSummary>> {
        let actions = self.catalog().list_actions();
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
        self.catalog()
            .get_action(action)
            .ok_or_else(|| anyhow!("unknown action: {action}"))
    }

    pub fn list_classes(&self) -> Result<Vec<ClassSummary>> {
        let live_objects = load_object_files(&self.paths)?
            .into_iter()
            .filter(|entry| entry.record.status == FileStatus::Live)
            .collect::<Vec<_>>();
        Ok(self
            .catalog()
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
            .catalog()
            .get_class(class)
            .ok_or_else(|| anyhow!("unknown class: {class}"))?;
        let live_count = load_object_files(&self.paths)?
            .into_iter()
            .filter(|entry| entry.record.status == FileStatus::Live && entry.record.class == *class)
            .count();
        Ok(ClassSummary {
            live_count,
            ..self.class_summary(class_info)
        })
    }

    pub fn check_action(&self, action: &QualifiedName) -> Result<CheckActionReport> {
        let action_summary = self
            .catalog()
            .get_action(action)
            .ok_or_else(|| anyhow!("unknown action: {action}"))?;
        let entries = load_object_files(&self.paths)?;
        let locked = crate::transactions::locked_inputs(&load_records(&self.paths)?);
        let held = self.holds.labels();
        let live_objects = entries
            .iter()
            .filter(|entry| {
                entry.record.status == FileStatus::Live
                    && !locked.contains(&entry.file_name)
                    && !held.contains_key(&entry.file_name)
            })
            .collect::<Vec<_>>();

        let mut available = Vec::new();
        let mut missing_inputs = Vec::new();
        let mut used_content_hashes = HashSet::new();

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
                    && !used_content_hashes.contains(&entry.record.content_hash)
                    && matches!(
                        (expected_hash, obj_type_hash(&entry.record.obj)),
                        (Some(expected), Some(actual)) if expected == actual
                    )
            });
            if let Some(entry) = candidate {
                used_content_hashes.insert(entry.record.content_hash);
                available.push(CheckActionCandidate {
                    class: required.class.clone(),
                    content_hash: encode_hash_hex(&entry.record.content_hash),
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

    pub fn get_state_root(&self) -> Result<Hash> {
        let settings = self.load_settings()?;
        let head = self
            .deps
            .synchronizer
            .fetch_head(&settings.synchronizer_api_url)?;
        Ok(head.state_root)
    }

    /// Import an external `.dobj` object — one not produced by this driver
    /// (e.g. a file from outside `~/.dobj/`) — into the local object store.
    ///
    /// `dobj_json` is the raw JSON contents of a `.dobj` file. The object is
    /// filed under `requested_file_name` when given (typically the name of
    /// the file it came in), otherwise under a name derived from its
    /// commitment. A requested name gets `.dobj` added when it lacks it and a
    /// numbered suffix when another file holds it, and is refused when unsafe
    /// as a file name or when it has the form of a derived name without being
    /// this object's.
    ///
    /// Validation:
    /// - the `class` must be one this driver's catalog knows, and the object's
    ///   committed `obj["type"]` predicate hash must equal that class's hash
    ///   (the same belt-and-suspenders check `execute` runs on its inputs);
    /// - the file's `contentHash` must be the commitment of its `obj`;
    /// - the object must not already be present (live or nullified);
    /// - the object's nullifier must not already be spent on-chain.
    ///
    /// Status is decided by grounding: `Live` if the source tx is committed,
    /// otherwise `Unknown` (a later `sync_objects` promotes it). If the
    /// synchronizer is unreachable the object is imported as `Unknown` rather
    /// than failing, so the flow still works offline.
    pub fn import_object(
        &self,
        dobj_json: &str,
        requested_file_name: Option<&str>,
    ) -> Result<ObjectSummary> {
        let mut record: ObjectRecord = serde_json::from_str(dobj_json).map_err(|err| {
            DriverError::InvalidInput(format!("could not parse .dobj contents: {err}"))
        })?;

        // 1. Class must be known, and the pod's committed `type` hash must
        //    match the catalog's class hash. A mismatch on either is fatal —
        //    the second check catches a `class` label that drifted from the
        //    object's actual pod-level identity.
        let class_info = self
            .catalog()
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

        // 2. The file's content hash must be the object's commitment, which
        //    is what the store and the chain know the object by. The file
        //    name is generated from the commitment, never the sender's.
        let commitment = record.obj.commitment();
        if record.content_hash != commitment {
            return Err(DriverError::InvalidInput(format!(
                "content hash mismatch: the file says {}, the object's is {}",
                encode_hash_hex(&record.content_hash),
                encode_hash_hex(&commitment)
            ))
            .into());
        }
        let generated_file_name = object_file_name(&record.class, &record.obj)
            .map_err(|err| DriverError::InvalidInput(format!("imported {err:#}")))?;

        // 3. Reject if we already hold this object or refuse its name, before
        //    asking the synchronizer. Checked again under the store lock
        //    before filing, since the store can change meanwhile.
        self.import_file_name(&record, &generated_file_name, requested_file_name)?;

        // 4. Grounding decides status. A nullifier already on-chain means the
        //    object has been spent — reject it. Otherwise Live if the object's
        //    commitment is in the created set, else Unknown. Tolerate
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
                    FileStatus::Live
                } else {
                    FileStatus::Unknown
                }
            }
            Err(err) => {
                log::warn!(
                    "import: grounding check failed, importing {generated_file_name} as unknown: {err:#}"
                );
                FileStatus::Unknown
            }
        };
        record.status = status;

        let file_name = {
            let _store = self.store_lock.lock().expect("store lock poisoned");
            let file_name =
                self.import_file_name(&record, &generated_file_name, requested_file_name)?;

            // Recorded before the object is filed, so a filed import is never
            // missing its fingerprint.
            if let Some(fingerprint) = key_fingerprint(&record.obj) {
                record_imported_key(&self.paths, &fingerprint)?;
            }

            // 5. File it. `write_object_file` routes to the live or nullified
            //    dir based on status, so a freshly-imported Unknown/Live lands
            //    live.
            write_object_file(&self.paths, &record, &file_name)?;
            file_name
        };

        self.summary_of(ObjectFileEntry { file_name, record })
    }

    /// The name an imported `record` is filed under: `requested`, adjusted
    /// as [`imported_file_name`] does, or `generated`. Refuses an object the
    /// store already holds, live or nullified, and a requested name it
    /// refuses. A requested name skips past names in the discarded
    /// directory too, so discarding it later cannot replace another file.
    fn import_file_name(
        &self,
        record: &ObjectRecord,
        generated: &str,
        requested: Option<&str>,
    ) -> Result<String> {
        let entries = load_object_files(&self.paths)?;
        if let Some(held) = entries
            .iter()
            .find(|entry| entry.record.content_hash == record.content_hash)
        {
            return Err(DriverError::Conflict(format!(
                "object already present: {}",
                held.file_name
            ))
            .into());
        }
        let file_name = match requested {
            None => generated.to_string(),
            Some(requested) => imported_file_name(requested, generated, |name| {
                store_has_file(&self.paths, name)
            })
            .map_err(|err| DriverError::InvalidInput(format!("{err:#}")))?,
        };
        if entries.iter().any(|entry| entry.file_name == file_name) {
            return Err(
                DriverError::Conflict(format!("object already present: {file_name}")).into(),
            );
        }
        Ok(file_name)
    }

    pub fn generated_podlang(&self) -> Option<String> {
        self.catalog().generated_podlang()
    }

    fn summary_of(&self, entry: ObjectFileEntry) -> Result<ObjectSummary> {
        let records = load_records(&self.paths)?;
        Ok(self
            .summaries(std::slice::from_ref(&entry), &records)
            .remove(0))
    }

    pub(crate) fn summaries<'a>(
        &self,
        entries: impl IntoIterator<Item = &'a ObjectFileEntry>,
        records: &[TransactionRecord],
    ) -> Vec<ObjectSummary> {
        let index = RecordIndex::new(records);
        let held = self.holds.labels();
        let catalog = self.catalog();
        let imported_keys = load_imported_keys(&self.paths).unwrap_or_else(|err| {
            log::warn!("treating no object keys as shared: {err:#}");
            HashSet::new()
        });
        entries
            .into_iter()
            .map(|entry| {
                self.object_summary(entry, &index, &held, &imported_keys, catalog.as_ref())
            })
            .collect()
    }

    /// Derives pending status and tx hash from the producing transaction.
    /// Reports the proof holding the file or the open transaction spending it.
    fn object_summary(
        &self,
        entry: &ObjectFileEntry,
        index: &RecordIndex<'_>,
        held: &HashMap<String, String>,
        imported_keys: &HashSet<Hash>,
        catalog: &dyn ActionCatalog,
    ) -> ObjectSummary {
        let class_info = catalog.get_class(&entry.record.class);
        let record = index.by_tx_final.get(&entry.record.tx_final);
        let status = match (entry.record.status, record.map(|record| record.state)) {
            (FileStatus::Unknown, Some(TransactionState::Submitted)) => ObjectStatus::Pending,
            (status, _) => status.into(),
        };
        let held_by_run_id = held.get(&entry.file_name).cloned().or_else(|| {
            index
                .locked_by
                .get(entry.file_name.as_str())
                .map(|owner| owner.to_string())
        });
        ObjectSummary {
            content_hash: encode_hash_hex(&entry.record.content_hash),
            file_name: entry.file_name.clone(),
            file_size: fs::metadata(self.resolve_managed_path(&entry.file_name))
                .ok()
                .map(|metadata| metadata.len()),
            class: entry.record.class.clone(),
            class_hash: class_info
                .as_ref()
                .map(|c| c.hash.clone())
                .unwrap_or_default(),
            emoji: class_info
                .as_ref()
                .map(|c| c.emoji.clone())
                .unwrap_or_else(|| "📦".to_string()),
            status,
            tx_hash: record
                .and_then(|record| record.tx_hash.clone())
                .or_else(|| entry.record.tx_hash.clone()),
            description: class_info.as_ref().map(|c| c.description.clone()),
            fields: entry.record.fields_map(),
            held_by_run_id,
            shared_key: key_fingerprint(&entry.record.obj)
                .is_some_and(|fingerprint| imported_keys.contains(&fingerprint)),
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
