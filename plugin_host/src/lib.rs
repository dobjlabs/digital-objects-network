//! Plugin host: loads `.pexe` plugins (manifest.toml + plugin.rhai) and provides
//! action/class metadata and proof-generation closures to the app.
//!
//! The manifest declares the static proof structure (podlang predicates).
//! The Rhai script contains runtime proof logic (executed at proof time).

mod recipes;

use std::collections::HashMap;

use anyhow::Result;
use craft_sdk::{ActionGroup, api};
use hex::FromHex;
use plugin_api::*;
use pod2::middleware::Hash;

/// Create a Rhai engine with sandbox limits.
fn create_engine() -> rhai::Engine {
    let mut engine = rhai::Engine::new();
    engine.set_max_operations(1_000_000);
    engine.set_max_call_levels(64);
    engine.set_max_string_size(10_000);
    engine.set_max_array_size(1_000);
    engine.set_max_map_size(100);
    engine
}

/// Hosts a loaded `.pexe` plugin and provides query + execution APIs.
pub struct PluginHost {
    metadata: PluginMetadata,
    /// Raw Rhai script source, stored for proof-time execution (Phase 2).
    /// Compiled to AST on demand since rhai::AST is not Sync.
    #[allow(dead_code)]
    script_source: String,
}

impl PluginHost {
    /// Load a plugin from a manifest TOML string and a Rhai script string.
    pub fn from_manifest_and_script(manifest_toml: &str, script_rhai: &str) -> Result<Self> {
        let manifest = Manifest::from_toml(manifest_toml)
            .map_err(|e| anyhow::anyhow!("failed to parse manifest: {e}"))?;
        let metadata: PluginMetadata = manifest.into();
        // Verify the script compiles (catch syntax errors at load time)
        let engine = create_engine();
        engine
            .compile(script_rhai)
            .map_err(|e| anyhow::anyhow!("failed to compile script: {e}"))?;
        Ok(Self {
            metadata,
            script_source: script_rhai.to_string(),
        })
    }

    /// Plugin name.
    pub fn name(&self) -> &str {
        &self.metadata.name
    }

    /// Raw metadata (useful for podlang inspection, etc.)
    pub fn metadata(&self) -> &PluginMetadata {
        &self.metadata
    }

    // -----------------------------------------------------------------------
    // Metadata queries (for UI, MCP, etc.)
    // -----------------------------------------------------------------------

    pub fn action_descriptors(&self) -> Vec<ActionDescriptor> {
        action_descriptors(&self.metadata)
    }

    pub fn visible_action_descriptors(&self) -> Vec<ActionDescriptor> {
        visible_action_descriptors(&self.metadata)
    }

    pub fn action_descriptors_by_name(&self) -> HashMap<String, ActionDescriptor> {
        action_descriptors_by_name(&self.metadata)
    }

    pub fn class_names(&self) -> Vec<String> {
        class_names(&self.metadata)
    }

    pub fn class_ui_meta(&self, class_name: &str) -> ClassUiMeta {
        class_ui_meta(&self.metadata, class_name)
    }

    // -----------------------------------------------------------------------
    // Proof generation: convert metadata into craft_sdk types
    // -----------------------------------------------------------------------

    /// Convert plugin dependency metadata into `craft_sdk::api::Dependency` values.
    pub fn dependencies(&self) -> Vec<api::Dependency> {
        self.metadata
            .dependencies
            .iter()
            .map(|dep| match dep.dep_type {
                DependencyType::Intro => api::Dependency::Intro {
                    pred: dep.pred.clone().leak(),
                    hash: Hash::from_hex(&dep.hash)
                        .unwrap_or_else(|_| panic!("invalid intro pod hash: {}", dep.hash)),
                },
                DependencyType::Module => api::Dependency::Module {
                    name: dep.pred.clone().leak(),
                    hash: Hash::from_hex(&dep.hash)
                        .unwrap_or_else(|_| panic!("invalid module hash: {}", dep.hash)),
                },
            })
            .collect()
    }

    /// Build fresh `Vec<api::Action>` with proof-generation closures from metadata.
    ///
    /// Must be called fresh each time proof generation is needed because the
    /// closures inside `Detail` are not `Clone`.
    pub fn actions(&self) -> Vec<api::Action> {
        recipes::metadata_to_actions(&self.metadata)
    }
}

// ---------------------------------------------------------------------------
// Multi-module orchestrator
// ---------------------------------------------------------------------------

/// The built-in action plugins, embedded at compile time as (manifest, script) pairs.
const BUILTIN_PLUGINS: &[(&str, &str)] = &[
    (
        include_str!("../../data/plugins/actions/find_log/manifest.toml"),
        include_str!("../../data/plugins/actions/find_log/plugin.rhai"),
    ),
    (
        include_str!("../../data/plugins/actions/craft_wood/manifest.toml"),
        include_str!("../../data/plugins/actions/craft_wood/plugin.rhai"),
    ),
    (
        include_str!("../../data/plugins/actions/craft_sticks/manifest.toml"),
        include_str!("../../data/plugins/actions/craft_sticks/plugin.rhai"),
    ),
    (
        include_str!("../../data/plugins/actions/craft_wood_pick/manifest.toml"),
        include_str!("../../data/plugins/actions/craft_wood_pick/plugin.rhai"),
    ),
    (
        include_str!("../../data/plugins/actions/use_wood_pick/manifest.toml"),
        include_str!("../../data/plugins/actions/use_wood_pick/plugin.rhai"),
    ),
    (
        include_str!("../../data/plugins/actions/stone_tools/manifest.toml"),
        include_str!("../../data/plugins/actions/stone_tools/plugin.rhai"),
    ),
];

/// Orchestrates multiple `.pexe` action plugins, each compiled to its own
/// pod2 module with cross-module references.
pub struct PluginOrchestrator {
    hosts: Vec<PluginHost>,
    /// Merged metadata across all plugins (for unified queries).
    merged_metadata: PluginMetadata,
}

impl PluginOrchestrator {
    /// Load the built-in action plugins.
    pub fn builtin() -> Result<Self> {
        Self::from_manifest_script_pairs(BUILTIN_PLUGINS)
    }

    /// Load multiple plugins from (manifest, script) pairs.
    pub fn from_manifest_script_pairs(pairs: &[(&str, &str)]) -> Result<Self> {
        let mut hosts = Vec::new();
        for (manifest, script) in pairs {
            hosts.push(PluginHost::from_manifest_and_script(manifest, script)?);
        }

        // Build merged metadata
        let merged_metadata = Self::merge_metadata(&hosts);

        Ok(Self {
            hosts,
            merged_metadata,
        })
    }

    fn merge_metadata(hosts: &[PluginHost]) -> PluginMetadata {
        let mut all_deps = Vec::new();
        let mut all_classes = Vec::new();
        let mut all_actions = Vec::new();
        let mut seen_dep_hashes = std::collections::HashSet::new();
        let mut seen_class_names = std::collections::HashSet::new();

        for host in hosts {
            let meta = host.metadata();
            for dep in &meta.dependencies {
                if seen_dep_hashes.insert(dep.hash.clone()) {
                    all_deps.push(dep.clone());
                }
            }
            for class in &meta.classes {
                if seen_class_names.insert(class.name.clone()) {
                    all_classes.push(class.clone());
                }
            }
            all_actions.extend(meta.actions.clone());
        }

        PluginMetadata {
            name: "minecraft-basics".to_string(),
            version: "0.1.0".to_string(),
            dependencies: all_deps,
            classes: all_classes,
            actions: all_actions,
            imports: Vec::new(),
        }
    }

    /// Build `ActionGroup`s for `Helper::new_multi_module()`.
    pub fn action_groups(&self) -> Vec<ActionGroup> {
        self.hosts
            .iter()
            .map(|host| {
                let meta = host.metadata();
                let alias = meta.name.replace('-', "_");
                ActionGroup {
                    name: meta.name.clone(),
                    alias,
                    dependencies: host.dependencies(),
                    actions: recipes::metadata_to_actions(meta),
                    class_names: meta.classes.iter().map(|c| c.name.clone()).collect(),
                    imports: meta.imports.clone(),
                }
            })
            .collect()
    }

    // -----------------------------------------------------------------------
    // Metadata queries (same API surface as PluginHost)
    // -----------------------------------------------------------------------

    pub fn action_descriptors(&self) -> Vec<ActionDescriptor> {
        action_descriptors(&self.merged_metadata)
    }

    pub fn visible_action_descriptors(&self) -> Vec<ActionDescriptor> {
        visible_action_descriptors(&self.merged_metadata)
    }

    pub fn action_descriptors_by_name(&self) -> HashMap<String, ActionDescriptor> {
        action_descriptors_by_name(&self.merged_metadata)
    }

    pub fn class_names(&self) -> Vec<String> {
        class_names(&self.merged_metadata)
    }

    pub fn class_ui_meta(&self, class_name: &str) -> ClassUiMeta {
        class_ui_meta(&self.merged_metadata, class_name)
    }

    /// Build fresh `Vec<api::Action>` with proof-generation closures.
    pub fn actions(&self) -> Vec<api::Action> {
        recipes::metadata_to_actions(&self.merged_metadata)
    }

    /// Convert plugin dependency metadata into `craft_sdk::api::Dependency` values.
    pub fn dependencies(&self) -> Vec<api::Dependency> {
        self.merged_metadata
            .dependencies
            .iter()
            .map(|dep| match dep.dep_type {
                DependencyType::Intro => api::Dependency::Intro {
                    pred: dep.pred.clone().leak(),
                    hash: Hash::from_hex(&dep.hash)
                        .unwrap_or_else(|_| panic!("invalid intro pod hash: {}", dep.hash)),
                },
                DependencyType::Module => api::Dependency::Module {
                    name: dep.pred.clone().leak(),
                    hash: Hash::from_hex(&dep.hash)
                        .unwrap_or_else(|_| panic!("invalid module hash: {}", dep.hash)),
                },
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_single_plugin() {
        let manifest = include_str!("../../data/plugins/actions/find_log/manifest.toml");
        let script = include_str!("../../data/plugins/actions/find_log/plugin.rhai");
        let host =
            PluginHost::from_manifest_and_script(manifest, script).expect("failed to load plugin");
        assert_eq!(host.name(), "find-log");
    }

    #[test]
    fn test_orchestrator_load() {
        let orch = PluginOrchestrator::builtin().expect("failed to load orchestrator");
        assert_eq!(orch.hosts.len(), 6);
    }

    #[test]
    fn test_orchestrator_classes() {
        let orch = PluginOrchestrator::builtin().unwrap();
        let classes = orch.class_names();
        assert!(classes.contains(&"Log".to_string()));
        assert!(classes.contains(&"Wood".to_string()));
        assert!(classes.contains(&"Stone".to_string()));
        assert!(classes.contains(&"Stick".to_string()));
        assert!(classes.contains(&"WoodPick".to_string()));
        assert!(classes.contains(&"StonePick".to_string()));
        assert_eq!(classes.len(), 6);
    }

    #[test]
    fn test_orchestrator_actions() {
        let orch = PluginOrchestrator::builtin().unwrap();
        let descriptors = orch.action_descriptors();
        let names: Vec<&str> = descriptors.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"FindLog"));
        assert!(names.contains(&"CraftWood"));
        assert!(names.contains(&"CraftSticks"));
        assert!(names.contains(&"CraftWoodPick"));
        assert!(names.contains(&"MineStoneWithWoodPick"));
        assert_eq!(descriptors.len(), 9);
    }

    #[test]
    fn test_visible_actions_excludes_hidden() {
        let orch = PluginOrchestrator::builtin().unwrap();
        let visible = orch.visible_action_descriptors();
        let hidden_names = ["UseWoodPick", "UseStonePick"];
        for d in &visible {
            assert!(
                !hidden_names.contains(&d.name.as_str()),
                "hidden action {} should not be visible",
                d.name
            );
        }
        assert_eq!(visible.len(), 7);
    }

    #[test]
    fn test_orchestrator_dependencies() {
        let orch = PluginOrchestrator::builtin().unwrap();
        let deps = orch.dependencies();
        assert_eq!(deps.len(), 2);
    }

    #[test]
    fn test_orchestrator_closures() {
        let orch = PluginOrchestrator::builtin().unwrap();
        let actions = orch.actions();
        assert_eq!(actions.len(), 9);
        assert_eq!(actions[0].name, "FindLog");
        assert_eq!(actions[1].name, "CraftWood");
    }

    #[test]
    fn test_class_ui_meta() {
        let orch = PluginOrchestrator::builtin().unwrap();
        let wood = orch.class_ui_meta("Wood");
        assert_eq!(wood.emoji, "🪵");
        let unknown = orch.class_ui_meta("Nonexistent");
        assert_eq!(unknown.emoji, "📦");
    }

    #[test]
    fn test_action_io_signatures() {
        let orch = PluginOrchestrator::builtin().unwrap();
        let by_name = orch.action_descriptors_by_name();

        let find_log = &by_name["FindLog"];
        assert!(find_log.input_classes.is_empty());
        assert_eq!(find_log.output_classes, vec!["Log"]);

        let craft_wood = &by_name["CraftWood"];
        assert_eq!(craft_wood.input_classes, vec!["Log"]);
        assert_eq!(craft_wood.output_classes, vec!["Wood"]);

        let craft_sticks = &by_name["CraftSticks"];
        assert_eq!(craft_sticks.input_classes, vec!["Wood"]);
        assert_eq!(craft_sticks.output_classes, vec!["Stick", "Stick"]);

        // MineStoneWithWoodPick depends on UseWoodPick, so it inherits WoodPick in + WoodPick + Stone out
        let mine = &by_name["MineStoneWithWoodPick"];
        assert_eq!(mine.input_classes, vec!["WoodPick"]);
        assert_eq!(mine.output_classes, vec!["WoodPick", "Stone"]);
    }

    #[test]
    fn test_orchestrator_imports() {
        let orch = PluginOrchestrator::builtin().unwrap();
        // find-log has no imports
        assert!(orch.hosts[0].metadata().imports.is_empty());
        // craft-wood imports find-log
        assert_eq!(orch.hosts[1].metadata().imports, vec!["find-log"]);
        // craft-wood-pick imports craft-wood and craft-sticks
        assert_eq!(
            orch.hosts[3].metadata().imports,
            vec!["craft-wood", "craft-sticks"]
        );
    }

    #[test]
    fn test_orchestrator_action_groups() {
        let orch = PluginOrchestrator::builtin().unwrap();
        let groups = orch.action_groups();
        assert_eq!(groups.len(), 6);
        assert_eq!(groups[0].name, "find-log");
        assert_eq!(groups[0].alias, "find_log");
        assert!(groups[0].imports.is_empty());
        assert_eq!(groups[1].name, "craft-wood");
        assert_eq!(groups[1].imports, vec!["find-log"]);
        // stone-tools is the merged module with 4 actions
        let stone = groups.iter().find(|g| g.name == "stone-tools").unwrap();
        assert_eq!(stone.actions.len(), 4);
        assert_eq!(stone.imports, vec!["use-wood-pick", "craft-sticks"]);
    }

    #[test]
    fn test_multi_module_compilation() {
        use craft_sdk::Helper;

        let orch = PluginOrchestrator::builtin().unwrap();
        let groups = orch.action_groups();
        let helper = Helper::new_multi_module(groups);

        // Should have 7 modules: txlib + 6 action modules
        // (modules field is private, but we can check via podlang_src)
        assert!(!helper.podlang_src.is_empty());

        // Each action should have a unique hash
        let hashes = helper.action_hashes();
        assert_eq!(hashes.len(), 9);

        // Each class should have a hash
        let class_hashes = helper.class_hashes();
        assert_eq!(class_hashes.len(), 6);
    }

    #[test]
    fn test_print_podlang() {
        let orch = PluginOrchestrator::builtin().unwrap();
        let groups = orch.action_groups();
        let helper = craft_sdk::Helper::new_multi_module(groups);
        // podlang_src contains all modules separated by "// ---"
        for (i, src) in helper.podlang_src.split("\n// ---\n").enumerate() {
            println!("=== Module {} ===\n{}", i, src);
        }
    }
}
