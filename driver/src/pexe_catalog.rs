//! Action catalog backed by `.pexe` plugin archives.
//!
//! At construction time, each installed `.pexe` is unpacked and its script compiled
//! via `Sdk::load_module_from_src_manifest` (which enforces the manifest's
//! `module_hash`). The compiled module is used to derive action/class hashes and
//! the podlang source shown in the GUI.
//!
//! Classes and actions are keyed by qualified id `<plugin_name>:<name>`. Two
//! plugins may declare a class or action with the same bare name; they are kept
//! distinct by qualified id and by their on-chain `Is{class}` predicate hash
//! (which differs between modules because each module has a unique
//! `module_hash`). Cross-plugin class references are not supported: an action
//! must reference classes declared in its own plugin.
//!
//! The compiled [`sdk::SdkModule`] is not kept — it holds a `Rc<Engine>` and is
//! therefore `!Send`. `execute_action` re-loads the script from its stored bytes
//! on demand, matching the per-call pattern used before.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use hex::FromHex;
use pod2::middleware::Hash;
use sdk::{Sdk, SpendableObject, SpendableObjects, manifest::Manifest};
use txlib::GroundingWitness;

use crate::catalog::{ActionCatalog, CatalogClass, extract_predicate};
use crate::types::ActionSummary;

struct Plugin {
    #[allow(dead_code)]
    path: PathBuf,
    manifest: Manifest,
    script: String,
    /// Qualified ids (`<plugin>:<action>`) provided by this plugin. Used to
    /// route `execute_action` back to the originating script bytes.
    action_ids: Vec<String>,
}

pub struct PexeCatalog {
    plugins: Vec<Plugin>,
    actions: Vec<ActionSummary>,
    actions_by_id: HashMap<String, ActionSummary>,
    /// Maps qualified action id -> plugin index in `plugins`.
    action_plugin_idx: HashMap<String, usize>,
    classes: Vec<CatalogClass>,
    classes_by_id: HashMap<String, CatalogClass>,
    classes_by_hash: HashMap<Hash, String>,
    /// Bare class/action names that appear in more than one plugin.
    name_collisions: Vec<String>,
    combined_podlang_src: String,
    mock_proofs: bool,
}

pub fn qualified_id(plugin_name: &str, name: &str) -> String {
    format!("{plugin_name}:{name}")
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
        // Reject duplicate plugin.name early: qualified ids are
        // `<plugin>:<name>` and would otherwise collide silently.
        let mut seen_plugin_names: HashMap<String, usize> = HashMap::new();
        for (idx, plugin) in plugins.iter().enumerate() {
            let name = &plugin.manifest.plugin.name;
            if let Some(prior) = seen_plugin_names.insert(name.clone(), idx) {
                return Err(anyhow!(
                    "duplicate plugin name {name:?}: already registered by {} (other entry at index {prior})",
                    plugins[prior].path.display(),
                ));
            }
        }

        let sdk = Sdk::default();

        let mut all_actions: Vec<ActionSummary> = Vec::new();
        let mut classes_in_order: Vec<CatalogClass> = Vec::new();
        let mut combined_podlang = String::new();
        // Track class hash by bare class name within each plugin so we can
        // resolve action input/output class refs without consulting other
        // plugins. (Within one module, bare class names are unique.)
        let mut enriched_plugins: Vec<Plugin> = Vec::with_capacity(plugins.len());
        let mut bare_name_counts: BTreeMap<String, usize> = BTreeMap::new();
        let mut action_plugin_idx: HashMap<String, usize> = HashMap::new();

        for mut plugin in plugins {
            let plugin_name = plugin.manifest.plugin.name.clone();
            let module = sdk
                .load_module_from_src_manifest(&plugin.script, &plugin.manifest)
                .map_err(|err| anyhow!("failed to load plugin {plugin_name}: {err}"))?;
            let podlang_src = module.podlang_src().to_string();
            if !combined_podlang.is_empty() {
                combined_podlang.push_str("\n// ---\n");
            }
            combined_podlang.push_str(&format!("// plugin: {plugin_name}\n{podlang_src}"));

            // Per-plugin class hash map. Module-scoped: a `Wood` class in
            // another plugin has a different IsWood predicate hash and lives
            // in a different `class_hashes` map below.
            let mut class_hashes: HashMap<String, Hash> = HashMap::new();
            for class in module.classes() {
                let hash = module.class_hash(&class.name).ok_or_else(|| {
                    anyhow!(
                        "plugin {plugin_name}: class {} has no compiled hash",
                        class.name
                    )
                })?;
                class_hashes.insert(class.name.clone(), hash);
            }

            // Build CatalogClass entries from this plugin's classes.
            let class_meta_by_name: HashMap<&str, &sdk::manifest::Class> = plugin
                .manifest
                .classes
                .iter()
                .map(|c| (c.name.as_str(), c))
                .collect();

            for class in module.classes() {
                let bare = &class.name;
                *bare_name_counts.entry(bare.clone()).or_insert(0) += 1;
                let qid = qualified_id(&plugin_name, bare);
                let class_hash = class_hashes[bare];
                let meta = class_meta_by_name.get(bare.as_str());
                let predicate_source = extract_predicate(&podlang_src, &format!("Is{bare}"))
                    .unwrap_or_else(|| format!("Is{bare}(state) = OR(...)"));
                classes_in_order.push(CatalogClass {
                    id: qid,
                    display_name: bare.clone(),
                    plugin_name: plugin_name.clone(),
                    emoji: meta.map_or("📦", |m| m.emoji.as_str()).to_string(),
                    hash: format!("{:#}", class_hash),
                    description: meta
                        .map_or("Unknown class object", |m| m.description.as_str())
                        .to_string(),
                    produced_by: Vec::new(), // filled in second pass
                    consumed_by: Vec::new(), // filled in second pass
                    predicate_source,
                });
            }

            // Build ActionSummary rows. Each input/output class is resolved
            // against this plugin's own class set; cross-plugin references
            // are rejected. Hidden actions are still recorded so their
            // qualified id routes back to this plugin via execute_action.
            let action_meta_by_name: HashMap<&str, &sdk::manifest::Action> = plugin
                .manifest
                .actions
                .iter()
                .map(|a| (a.name.as_str(), a))
                .collect();
            let plugin_idx = enriched_plugins.len();
            let mut plugin_action_ids: Vec<String> = Vec::new();

            for action in module.actions() {
                let bare = action.name.clone();
                *bare_name_counts.entry(bare.clone()).or_insert(0) += 1;
                let qid = qualified_id(&plugin_name, &bare);
                plugin_action_ids.push(qid.clone());
                if let Some(prior) = action_plugin_idx.insert(qid.clone(), plugin_idx) {
                    return Err(anyhow!(
                        "internal: duplicate action qualified id {qid} (already mapped to plugin idx {prior})"
                    ));
                }

                let meta = action_meta_by_name.get(bare.as_str());
                let resolve_class = |class_name: &str| -> Result<(String, String, String)> {
                    let hash = class_hashes.get(class_name).ok_or_else(|| {
                        anyhow!(
                            "plugin {plugin_name}: action {bare} references class {class_name:?} \
                             which is not declared in this plugin (cross-plugin class \
                             references are not supported yet)"
                        )
                    })?;
                    Ok((
                        qualified_id(&plugin_name, class_name),
                        class_name.to_string(),
                        format!("{:#}", hash),
                    ))
                };

                let mut total_input_class_ids = Vec::new();
                let mut total_input_class_names = Vec::new();
                let mut total_input_class_hashes = Vec::new();
                for r in action.total_inputs() {
                    let (id, name, hash) = resolve_class(&r.class)?;
                    total_input_class_ids.push(id);
                    total_input_class_names.push(name);
                    total_input_class_hashes.push(hash);
                }

                let mut total_output_class_ids = Vec::new();
                let mut total_output_class_names = Vec::new();
                let mut total_output_class_hashes = Vec::new();
                for r in action.total_outputs() {
                    let (id, name, hash) = resolve_class(&r.class)?;
                    total_output_class_ids.push(id);
                    total_output_class_names.push(name);
                    total_output_class_hashes.push(hash);
                }

                if meta.is_some_and(|m| m.hidden) {
                    continue;
                }

                let action_hash = module
                    .action_hash(&bare)
                    .map(|h| format!("{:#}", h))
                    .unwrap_or_default();
                all_actions.push(ActionSummary {
                    id: qid,
                    display_name: bare.clone(),
                    plugin_name: plugin_name.clone(),
                    emoji: meta.map_or("⚙️", |m| m.emoji.as_str()).to_string(),
                    hash: action_hash,
                    description: meta
                        .map_or("Pexe action", |m| m.description.as_str())
                        .to_string(),
                    total_input_class_ids,
                    total_input_class_names,
                    total_input_class_hashes,
                    total_output_class_ids,
                    total_output_class_names,
                    total_output_class_hashes,
                });
            }

            plugin.action_ids = plugin_action_ids;
            enriched_plugins.push(plugin);
        }

        // Second pass: fill produced_by / consumed_by per class using qualified ids.
        for class in classes_in_order.iter_mut() {
            class.produced_by = all_actions
                .iter()
                .filter(|a| a.total_output_class_ids.contains(&class.id))
                .map(|a| a.id.clone())
                .collect();
            class.consumed_by = all_actions
                .iter()
                .filter(|a| a.total_input_class_ids.contains(&class.id))
                .map(|a| a.id.clone())
                .collect();
        }

        // Deterministic GUI order: sort by display name, then plugin.
        classes_in_order.sort_by(|a, b| {
            a.display_name
                .cmp(&b.display_name)
                .then_with(|| a.plugin_name.cmp(&b.plugin_name))
        });

        let actions_by_id: HashMap<String, ActionSummary> = all_actions
            .iter()
            .map(|a| (a.id.clone(), a.clone()))
            .collect();
        let classes_by_id: HashMap<String, CatalogClass> = classes_in_order
            .iter()
            .map(|c| (c.id.clone(), c.clone()))
            .collect();
        let classes_by_hash: HashMap<Hash, String> = classes_in_order
            .iter()
            .filter_map(|c| {
                parse_hash_hex(&c.hash)
                    .ok()
                    .map(|hash| (hash, c.id.clone()))
            })
            .collect();
        let name_collisions: Vec<String> = bare_name_counts
            .into_iter()
            .filter_map(|(name, count)| (count > 1).then_some(name))
            .collect();

        Ok(Self {
            plugins: enriched_plugins,
            actions: all_actions,
            actions_by_id,
            action_plugin_idx,
            classes: classes_in_order,
            classes_by_id,
            classes_by_hash,
            name_collisions,
            combined_podlang_src: combined_podlang,
            mock_proofs,
        })
    }

    pub fn plugin_count(&self) -> usize {
        self.plugins.len()
    }
}

impl ActionCatalog for PexeCatalog {
    fn list_actions(&self) -> Vec<ActionSummary> {
        self.actions.clone()
    }

    fn get_action(&self, action_id: &str) -> Option<ActionSummary> {
        self.actions_by_id.get(action_id).cloned()
    }

    fn list_classes(&self) -> Vec<CatalogClass> {
        self.classes.clone()
    }

    fn get_class(&self, class_id: &str) -> Option<CatalogClass> {
        self.classes_by_id.get(class_id).cloned()
    }

    fn get_class_by_hash(&self, class_hash: &Hash) -> Option<CatalogClass> {
        let id = self.classes_by_hash.get(class_hash)?;
        self.classes_by_id.get(id).cloned()
    }

    fn name_collisions(&self) -> Vec<String> {
        self.name_collisions.clone()
    }

    fn execute_action(
        &self,
        action_id: String,
        grounding_witness: GroundingWitness,
        inputs: Vec<SpendableObject>,
    ) -> Result<SpendableObjects> {
        let plugin_idx = *self
            .action_plugin_idx
            .get(&action_id)
            .ok_or_else(|| anyhow!("no plugin provides action {action_id}"))?;
        let plugin = &self.plugins[plugin_idx];
        let bare_name = bare_action_name(&action_id, &plugin.manifest.plugin.name)
            .ok_or_else(|| anyhow!("invalid qualified action id: {action_id}"))?;
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
        Ok(executor.action(bare_name, inputs)?)
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
        action_ids: Vec::new(),
    })
}

fn bare_action_name<'a>(qualified_id: &'a str, plugin_name: &str) -> Option<&'a str> {
    let prefix_len = plugin_name.len();
    if qualified_id.len() <= prefix_len + 1 {
        return None;
    }
    let (head, rest) = qualified_id.split_at(prefix_len);
    if head != plugin_name || !rest.starts_with(':') {
        return None;
    }
    Some(&rest[1..])
}

fn parse_hash_hex(s: &str) -> Result<Hash> {
    let trimmed = s.strip_prefix("0x").unwrap_or(s);
    Hash::from_hex(trimmed).map_err(|err| anyhow!("invalid hash hex {s:?}: {err}"))
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
        assert!(action_ids.contains(&"craft-basics:CraftWood".to_string()));
        assert!(!action_ids.contains(&"craft-basics:UseWoodPick".to_string()));
    }

    #[test]
    fn test_pexe_catalog_lists_classes() {
        let catalog = test_catalog();
        let class_ids: Vec<_> = catalog.list_classes().into_iter().map(|c| c.id).collect();
        assert!(class_ids.contains(&"craft-basics:Log".to_string()));
        assert!(class_ids.contains(&"craft-basics:WoodPick".to_string()));
    }

    #[test]
    fn test_pexe_catalog_empty_dir_has_no_plugins() {
        let catalog = PexeCatalog::from_bytes(std::iter::empty(), true).unwrap();
        assert_eq!(catalog.plugin_count(), 0);
        assert!(catalog.list_actions().is_empty());
        assert!(catalog.generated_podlang().is_none());
    }

    #[test]
    fn test_get_class_by_hash_round_trip() {
        let catalog = test_catalog();
        let log = catalog
            .get_class("craft-basics:Log")
            .expect("Log class present");
        let by_hash = parse_hash_hex(&log.hash)
            .ok()
            .and_then(|h| catalog.get_class_by_hash(&h))
            .expect("class hash resolves back");
        assert_eq!(by_hash.id, log.id);
    }

    #[test]
    fn test_no_collisions_for_single_plugin() {
        let catalog = test_catalog();
        assert!(catalog.name_collisions().is_empty());
    }

    #[test]
    fn test_duplicate_plugin_name_rejected() {
        let result = PexeCatalog::from_bytes(
            [
                (PathBuf::from("a.pexe"), test_plugin_bytes()),
                (PathBuf::from("b.pexe"), test_plugin_bytes()),
            ],
            true,
        );
        match result {
            Ok(_) => panic!("expected duplicate-plugin-name error, but load succeeded"),
            Err(err) => assert!(
                err.to_string().contains("duplicate plugin name"),
                "expected duplicate-plugin-name error, got: {err}"
            ),
        }
    }
}
