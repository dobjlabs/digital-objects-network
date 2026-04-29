//! Action catalog backed by `.pexe` plugin archives.
//!
//! At construction time, each installed `.pexe` is unpacked and its script compiled
//! via `Sdk::load_module_from_src_manifest` (which enforces the manifest's
//! `module_hash`). The compiled module is used to derive action/class hashes and
//! the podlang source shown in the GUI.
//!
//! The compiled [`sdk::SdkModule`] is not kept — it holds a `Rc<Engine>` and is
//! therefore `!Send`. `execute_action` re-loads the script from its stored bytes
//! on demand, matching the per-call pattern used before.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use sdk::{manifest::Manifest, Sdk, SpendableObject, SpendableObjects};
use txlib::GroundingWitness;

use crate::catalog::{extract_predicate, ActionCatalog, CatalogClass};
use crate::types::ActionSummary;

struct Plugin {
    #[allow(dead_code)]
    path: PathBuf,
    manifest: Manifest,
    script: String,
    podlang_src: String,
    /// Names of actions this plugin provides (for lookup by name).
    action_names: Vec<String>,
}

pub struct PexeCatalog {
    plugins: Vec<Plugin>,
    actions: Vec<ActionSummary>,
    actions_by_name: HashMap<String, ActionSummary>,
    classes: Vec<CatalogClass>,
    classes_by_name: HashMap<String, CatalogClass>,
    combined_podlang_src: String,
    mock_proofs: bool,
}

impl PexeCatalog {
    /// Scan `actions_dir` for `.pexe` files, unpack them, and assemble the catalog.
    pub fn load(actions_dir: &Path) -> Result<Self> {
        let plugins = discover_plugins(actions_dir)?;
        Self::from_plugins(plugins, false)
    }

    /// Assemble a catalog from already-loaded plugin sources. Used by tests that
    /// pack plugin bytes in-memory.
    pub fn from_bytes<I>(pexe_entries: I, mock_proofs: bool) -> Result<Self>
    where
        I: IntoIterator<Item = (PathBuf, Vec<u8>)>,
    {
        let mut plugins = Vec::new();
        for (path, bytes) in pexe_entries {
            plugins.push(load_plugin_from_bytes(path, &bytes)?);
        }
        Self::from_plugins(plugins, mock_proofs)
    }

    fn from_plugins(plugins: Vec<Plugin>, mock_proofs: bool) -> Result<Self> {
        let sdk = Sdk::default();

        // Compile each plugin's module to derive action/class hashes and podlang.
        // Discard the Rc<SdkModule> afterwards (Rhai Engine is !Send).
        let mut all_actions: Vec<ActionSummary> = Vec::new();
        let mut all_classes_ordered: Vec<String> = Vec::new();
        let mut class_meta: HashMap<String, (String, String)> = HashMap::new(); // name -> (emoji, desc)
        let mut class_hashes: HashMap<String, String> = HashMap::new();
        let mut action_signatures: HashMap<String, (Vec<String>, Vec<String>)> = HashMap::new();
        let mut combined_podlang = String::new();

        let mut enriched_plugins: Vec<Plugin> = Vec::with_capacity(plugins.len());
        for mut plugin in plugins {
            let module = sdk
                .load_module_from_src_manifest(&plugin.script, &plugin.manifest)
                .map_err(|err| {
                    anyhow!(
                        "failed to load plugin {}: {err}",
                        plugin.manifest.plugin.name
                    )
                })?;
            let podlang_src = module.podlang_src().to_string();
            if !combined_podlang.is_empty() {
                combined_podlang.push_str("\n// ---\n");
            }
            combined_podlang.push_str(&format!(
                "// plugin: {}\n{}",
                plugin.manifest.plugin.name, podlang_src
            ));
            plugin.podlang_src = podlang_src;

            // Capture class metadata from the manifest.
            for class in &plugin.manifest.classes {
                if !class_meta.contains_key(&class.name) {
                    class_meta.insert(
                        class.name.clone(),
                        (class.emoji.clone(), class.description.clone()),
                    );
                }
            }

            // Compute hashes + signatures from the compiled module, and turn
            // actions into ActionSummary rows.
            let action_meta_by_name: HashMap<&str, &sdk::manifest::Action> = plugin
                .manifest
                .actions
                .iter()
                .map(|a| (a.name.as_str(), a))
                .collect();

            plugin.action_names = module.actions().iter().map(|a| a.name.clone()).collect();

            for action in module.actions() {
                let name = action.name.as_str();
                let total_input_classes: Vec<String> = action
                    .total_inputs
                    .iter()
                    .map(|(_o, c)| c.clone())
                    .collect();
                let total_output_classes: Vec<String> = action
                    .total_outputs
                    .iter()
                    .map(|(_o, c)| c.clone())
                    .collect();
                action_signatures.insert(
                    name.to_string(),
                    (total_input_classes.clone(), total_output_classes.clone()),
                );

                let meta = action_meta_by_name.get(name);
                if meta.is_some_and(|m| m.hidden) {
                    continue;
                }
                let action_hash = module
                    .action_hash(name)
                    .map(|h| format!("{:#}", h))
                    .unwrap_or_default();
                all_actions.push(ActionSummary {
                    id: name.to_string(),
                    emoji: meta.map_or("⚙️", |m| m.emoji.as_str()).to_string(),
                    hash: action_hash,
                    total_input_class_hashes: Vec::new(), // filled in a second pass
                    description: meta
                        .map_or("Pexe action", |m| m.description.as_str())
                        .to_string(),
                    total_input_classes,
                    total_output_classes,
                });
            }

            for class in module.classes() {
                let name = &class.name;
                if !all_classes_ordered.contains(name) {
                    all_classes_ordered.push(name.clone());
                }
                let hash = module
                    .class_hash(name)
                    .map(|h| format!("{:#}", h))
                    .unwrap_or_default();
                class_hashes.insert(name.clone(), hash);
            }

            enriched_plugins.push(plugin);
        }

        // Second pass: fill in input_class_hashes now that every class hash is known.
        for action in &mut all_actions {
            action.total_input_class_hashes = action
                .total_input_classes
                .iter()
                .map(|c| class_hashes.get(c).cloned().unwrap_or_default())
                .collect();
        }

        // Ensure manifest-declared classes that weren't seen via the compiled
        // module (shouldn't happen today, but be defensive) still show up.
        for class_name in class_meta.keys() {
            if !all_classes_ordered.contains(class_name) {
                all_classes_ordered.push(class_name.clone());
            }
        }
        // Deterministic order: alphabetical for the GUI.
        let class_names_sorted: BTreeSet<String> = all_classes_ordered.into_iter().collect();

        let classes: Vec<CatalogClass> = class_names_sorted
            .into_iter()
            .map(|class_name| {
                let (emoji, description) = class_meta
                    .get(&class_name)
                    .cloned()
                    .unwrap_or_else(|| ("📦".to_string(), "Unknown class object".to_string()));
                let produced_by = all_actions
                    .iter()
                    .filter(|a| a.total_output_classes.contains(&class_name))
                    .map(|a| a.id.clone())
                    .collect();
                let consumed_by = all_actions
                    .iter()
                    .filter(|a| a.total_input_classes.contains(&class_name))
                    .map(|a| a.id.clone())
                    .collect();
                let predicate_source =
                    extract_predicate(&combined_podlang, &format!("Is{class_name}"))
                        .unwrap_or_else(|| format!("Is{class_name}(state) = OR(...)"));
                CatalogClass {
                    name: class_name.clone(),
                    emoji,
                    hash: class_hashes.get(&class_name).cloned().unwrap_or_default(),
                    description,
                    produced_by,
                    consumed_by,
                    predicate_source,
                }
            })
            .collect();

        let actions_by_name: HashMap<String, ActionSummary> = all_actions
            .iter()
            .map(|a| (a.id.clone(), a.clone()))
            .collect();
        let classes_by_name: HashMap<String, CatalogClass> = classes
            .iter()
            .map(|c| (c.name.clone(), c.clone()))
            .collect();

        Ok(Self {
            plugins: enriched_plugins,
            actions: all_actions,
            actions_by_name,
            classes,
            classes_by_name,
            combined_podlang_src: combined_podlang,
            mock_proofs,
        })
    }

    pub fn plugin_count(&self) -> usize {
        self.plugins.len()
    }

    fn find_plugin_for(&self, action_id: &str) -> Option<&Plugin> {
        self.plugins
            .iter()
            .find(|p| p.action_names.iter().any(|n| n == action_id))
    }
}

impl ActionCatalog for PexeCatalog {
    fn list_actions(&self) -> Vec<ActionSummary> {
        self.actions.clone()
    }

    fn get_action(&self, action_id: &str) -> Option<ActionSummary> {
        self.actions_by_name.get(action_id).cloned()
    }

    fn list_classes(&self) -> Vec<CatalogClass> {
        self.classes.clone()
    }

    fn get_class(&self, class_name: &str) -> Option<CatalogClass> {
        self.classes_by_name.get(class_name).cloned()
    }

    fn execute_action(
        &self,
        action_id: String,
        grounding_witness: GroundingWitness,
        inputs: Vec<SpendableObject>,
    ) -> Result<SpendableObjects> {
        let plugin = self
            .find_plugin_for(&action_id)
            .ok_or_else(|| anyhow!("no plugin provides action {action_id}"))?;
        let sdk = Sdk::default();
        let module = sdk
            .load_module_from_src_manifest(&plugin.script, &plugin.manifest)
            .map_err(|err| {
                anyhow!(
                    "failed to reload plugin {} for execution: {err}",
                    plugin.manifest.plugin.name
                )
            })?;
        let executor = module.executor(self.mock_proofs, Arc::new(grounding_witness));
        Ok(executor.action(&action_id, inputs)?)
    }

    fn generated_podlang(&self) -> Option<String> {
        if self.combined_podlang_src.is_empty() {
            None
        } else {
            Some(self.combined_podlang_src.clone())
        }
    }
}

fn discover_plugins(actions_dir: &Path) -> Result<Vec<Plugin>> {
    if !actions_dir.exists() {
        return Ok(Vec::new());
    }
    let mut entries: Vec<PathBuf> = std::fs::read_dir(actions_dir)
        .with_context(|| format!("failed to read {}", actions_dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some(pexe::PEXE_EXTENSION))
        .collect();
    entries.sort();

    let mut plugins = Vec::with_capacity(entries.len());
    for path in entries {
        let bytes = std::fs::read(&path)
            .with_context(|| format!("failed to read pexe {}", path.display()))?;
        plugins.push(load_plugin_from_bytes(path, &bytes)?);
    }
    Ok(plugins)
}

fn load_plugin_from_bytes(path: PathBuf, bytes: &[u8]) -> Result<Plugin> {
    let (manifest, script) = pexe::unpack(bytes)
        .map_err(|err| anyhow!("failed to unpack pexe {}: {err}", path.display()))?;
    Ok(Plugin {
        path,
        manifest,
        script,
        podlang_src: String::new(),
        action_names: Vec::new(),
    })
}

#[cfg(test)]
pub(crate) fn test_plugin_bytes() -> Vec<u8> {
    // Pack the live plugin sources in-memory so tests never touch ~/.dobj/actions.
    let manifest = include_str!("../../plugins/craft-basics/manifest.toml");
    let script = include_str!("../../plugins/craft-basics/plugin.rhai");
    pexe::pack(manifest, script).expect("test plugin packs")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_catalog() -> PexeCatalog {
        PexeCatalog::from_bytes(
            std::iter::once((PathBuf::from("craft-basics.pexe"), test_plugin_bytes())),
            true,
        )
        .unwrap()
    }

    #[test]
    fn test_pexe_catalog_hides_internal_actions() {
        let catalog = test_catalog();
        let action_ids: Vec<_> = catalog.list_actions().into_iter().map(|a| a.id).collect();
        assert!(action_ids.contains(&"CraftWood".to_string()));
        assert!(!action_ids.contains(&"UseWoodPick".to_string()));
    }

    #[test]
    fn test_pexe_catalog_lists_classes() {
        let catalog = test_catalog();
        let class_names: Vec<_> = catalog.list_classes().into_iter().map(|c| c.name).collect();
        assert!(class_names.contains(&"Log".to_string()));
        assert!(class_names.contains(&"WoodPick".to_string()));
    }

    #[test]
    fn test_pexe_catalog_empty_dir_has_no_plugins() {
        let catalog = PexeCatalog::from_bytes(std::iter::empty(), true).unwrap();
        assert_eq!(catalog.plugin_count(), 0);
        assert!(catalog.list_actions().is_empty());
        assert!(catalog.generated_podlang().is_none());
    }
}
