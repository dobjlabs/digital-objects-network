//! The local side of a trade: the user's objects and plugins from the
//! `$DOBJ_HOME` tree, service URLs from the daemon settings, and the
//! import of a received object back through dobjd.
//!
//! Reads go straight to the files (dobjd's read API serves summaries,
//! not the full dictionaries a proving client needs); the one write,
//! adopting the received object, goes through dobjd so its validation
//! and store stay authoritative.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context as _, Result, anyhow};
use driver::{ObjectRecord, parse_object_record_file, paths};
use pod2::{lang::Module, middleware::Hash};
use wire_types::{DriverSettings, ObjectStatus, QualifiedName};

use crate::engine::ClassDirectory;

pub struct ClassDisplay {
    pub qualified: QualifiedName,
    pub emoji: String,
}

pub struct PluginCatalog {
    /// Everything a `BuildContext` needs: the txlib batches plus every
    /// installed plugin's module.
    pub modules: Vec<Arc<Module>>,
    pub classes: ClassDirectory,
    pub display: HashMap<Hash, ClassDisplay>,
    by_name: HashMap<String, Hash>,
}

impl PluginCatalog {
    pub fn resolve_class(&self, name: &str) -> Result<Hash> {
        self.by_name.get(name).copied().ok_or_else(|| {
            let mut known: Vec<_> = self.by_name.keys().cloned().collect();
            known.sort();
            anyhow!(
                "no installed class named {name}; installed: {}",
                known.join(", ")
            )
        })
    }

    pub fn class_label(&self, hash: Hash) -> String {
        match self.display.get(&hash) {
            Some(display) => format!("{} {}", display.emoji, display.qualified),
            None => format!("(unknown class {hash:#})"),
        }
    }
}

pub struct Local {
    pub paths: driver::DriverPaths,
    pub settings: DriverSettings,
    pub catalog: PluginCatalog,
    pub dobjd_url: String,
}

impl Local {
    pub fn open() -> Result<Self> {
        let paths = paths::default_paths()?;
        let settings = read_settings(&paths)?;
        let catalog = load_plugins(&paths)?;
        let port = match std::env::var("DOBJD_PORT") {
            Ok(raw) => raw
                .parse::<u16>()
                .map_err(|err| anyhow!("invalid DOBJD_PORT env: {err}"))?,
            Err(_) => 7717,
        };
        Ok(Self {
            paths,
            settings,
            catalog,
            dobjd_url: format!("http://127.0.0.1:{port}"),
        })
    }

    /// Every live object in the store, full dictionaries included.
    pub fn live_objects(&self) -> Result<Vec<ObjectRecord>> {
        let mut records = Vec::new();
        let entries = match std::fs::read_dir(&self.paths.objects_dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(records),
            Err(err) => {
                return Err(anyhow!(
                    "cannot read objects dir {}: {err}",
                    self.paths.objects_dir.display()
                ));
            }
        };
        for entry in entries {
            let path = entry?.path();
            if !paths::is_dobj_file(&path) {
                continue;
            }
            let record = parse_object_record_file(&path)
                .with_context(|| format!("unreadable object file {}", path.display()))?;
            if record.status == ObjectStatus::Live {
                records.push(record);
            }
        }
        Ok(records)
    }

    /// Live objects of one class, by guard hash.
    pub fn live_objects_of_class(&self, class_hash: Hash) -> Result<Vec<ObjectRecord>> {
        let class_value = pod2::middleware::Value::from(class_hash);
        Ok(self
            .live_objects()?
            .into_iter()
            .filter(|record| {
                record
                    .obj
                    .get(&pod2::middleware::StrKey::from("type"))
                    .ok()
                    .flatten()
                    .is_some_and(|value| value == class_value)
            })
            .collect())
    }

    /// Adopt a received object through dobjd, which revalidates class
    /// identity and on-chain grounding and files it canonically.
    pub fn import_received(
        &self,
        obj: &pod2::middleware::containers::Dictionary,
        class_hash: Hash,
        tx_final: Hash,
    ) -> Result<()> {
        let display = self
            .catalog
            .display
            .get(&class_hash)
            .ok_or_else(|| anyhow!("received object's class is not installed"))?;
        let record = serde_json::json!({
            "contentHash": obj.commitment(),
            "class": display.qualified,
            "status": "live",
            "obj": obj,
            "txFinal": tx_final,
        });
        let body = serde_json::json!({ "dobj": record.to_string() });
        let url = format!("{}/objects/import", self.dobjd_url);
        let response = reqwest::blocking::Client::new()
            .post(&url)
            .json(&body)
            .send()
            .with_context(|| format!("dobjd unreachable at {url}"))?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().unwrap_or_default();
            anyhow::bail!("dobjd import failed with {status}: {text}");
        }
        Ok(())
    }
}

/// A grounding witness fabricated locally: the inputs become the whole
/// created set of a synthetic state. Only for `--mock` dry runs, where
/// nothing is posted; it lets the full protocol run with no
/// synchronizer at all.
pub fn mock_grounding_witness(commitments: &[Hash]) -> Arc<txlib::GroundingWitness> {
    let mut state = payload::test_state::TestState::default();
    state.apply_tx(commitments.to_vec(), Vec::new());
    state.build_grounding_witness(
        commitments,
        |block_meta, created_root, nullifiers_root, prior_state_history_root, created_proofs| {
            Arc::new(txlib::GroundingWitness::new(
                txlib::StateHeader::new(
                    block_meta.number as i64,
                    block_meta.timestamp as i64,
                    block_meta.hash,
                    created_root,
                    nullifiers_root,
                    prior_state_history_root,
                ),
                created_proofs,
            ))
        },
    )
}

/// Fabricate a live object of `class_hash` and file it in the local
/// store. Dev scaffolding for mock runs: the object is not grounded in
/// any real state, so only `--mock` trades can move it.
pub fn dev_spawn(local: &Local, class_hash: Hash) -> Result<ObjectRecord> {
    let display = local
        .catalog
        .display
        .get(&class_hash)
        .ok_or_else(|| anyhow!("class is not installed"))?;
    let initial = pod2utils::dict!({
        "type" => pod2::middleware::Value::from(class_hash),
        "key" => pod2utils::rand_raw_value()
    });
    let obj = txlib::with_stable_identifier(&initial);
    let record = ObjectRecord {
        content_hash: obj.commitment(),
        class: display.qualified.clone(),
        status: ObjectStatus::Live,
        tx_hash: None,
        obj,
        tx_final: Hash::default(),
    };
    std::fs::create_dir_all(&local.paths.objects_dir)?;
    let file_name = format!(
        "{}_{}.dobj",
        record.class.file_prefix(),
        format!("{:#}", record.content_hash).to_ascii_lowercase()
    );
    let path = local.paths.objects_dir.join(file_name);
    std::fs::write(&path, serde_json::to_string_pretty(&record)?)?;
    Ok(record)
}

impl Local {
    /// Fetch object statuses through dobjd. The fetch itself runs the
    /// daemon's sync, so held objects reconcile against the chain
    /// (unknown -> live once created, live -> nullified once spent).
    pub fn object_statuses(&self) -> Result<HashMap<Hash, ObjectStatus>> {
        let url = format!("{}/objects", self.dobjd_url);
        let summaries: Vec<wire_types::ObjectSummary> = reqwest::blocking::Client::new()
            .get(&url)
            .send()
            .with_context(|| format!("dobjd unreachable at {url}"))?
            .error_for_status()
            .context("dobjd objects fetch failed")?
            .json()
            .context("unreadable dobjd objects response")?;
        let mut statuses = HashMap::new();
        for summary in summaries {
            if let Ok(hash) = payload::decode_hash_hex(&summary.content_hash) {
                statuses.insert(hash, summary.status);
            }
        }
        Ok(statuses)
    }
}

fn read_settings(paths: &driver::DriverPaths) -> Result<DriverSettings> {
    match std::fs::read_to_string(&paths.settings_path) {
        Ok(raw) => serde_json::from_str(&raw).map_err(|err| {
            anyhow!(
                "unreadable settings file {}: {err}",
                paths.settings_path.display()
            )
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(DriverSettings {
            synchronizer_api_url: "http://127.0.0.1:3000".to_string(),
            relayer_api_url: "http://127.0.0.1:3200".to_string(),
            mcp_enabled: false,
        }),
        Err(err) => Err(anyhow!(
            "cannot read settings file {}: {err}",
            paths.settings_path.display()
        )),
    }
}

fn load_plugins(paths: &driver::DriverPaths) -> Result<PluginCatalog> {
    let mut modules = vec![
        Arc::new(txlib::predicates::events_module()),
        Arc::new(txlib::predicates::rekey_module()),
        Arc::new(txlib::predicates::module()),
    ];
    let mut classes = ClassDirectory::default();
    let mut display = HashMap::new();
    let mut by_name = HashMap::new();

    let entries = match std::fs::read_dir(&paths.actions_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            anyhow::bail!(
                "no plugins installed under {} (run `just ensure-plugins`)",
                paths.actions_dir.display()
            );
        }
        Err(err) => {
            return Err(anyhow!(
                "cannot read actions dir {}: {err}",
                paths.actions_dir.display()
            ));
        }
    };
    let sdk = sdk::Sdk::default();
    for entry in entries {
        let path = entry?.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some(pexe::PEXE_EXTENSION) {
            continue;
        }
        let bytes = pexe::read_pexe_file(&path)
            .with_context(|| format!("unreadable plugin {}", path.display()))?;
        let (manifest, script) = pexe::unpack(&bytes)
            .with_context(|| format!("cannot unpack plugin {}", path.display()))?;
        let module = sdk
            .load_module_from_src_manifest(&script, &manifest)
            .map_err(|err| anyhow!("plugin {} does not load: {err}", manifest.plugin.name))?;
        for class in module.classes() {
            let hash = module
                .class_hash(&class.name)
                .expect("loaded module resolves its own classes");
            let emoji = manifest
                .classes
                .iter()
                .find(|c| c.name == class.name)
                .map(|c| c.emoji.clone())
                .unwrap_or_default();
            let qualified = QualifiedName::new(&manifest.plugin.name, &class.name);
            by_name.insert(class.name.clone(), hash);
            by_name.insert(qualified.to_string(), hash);
            display.insert(hash, ClassDisplay { qualified, emoji });
        }
        classes.absorb(ClassDirectory::from_sdk_module(&module));
        modules.push(module.module().clone());
    }
    Ok(PluginCatalog {
        modules,
        classes,
        display,
        by_name,
    })
}
