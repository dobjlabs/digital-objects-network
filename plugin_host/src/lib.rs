//! Plugin host: loads `.pexe` WASM modules via Extism and provides
//! action/class metadata and proof-generation closures to the app.

mod recipes;

use std::collections::HashMap;

use anyhow::Result;
use craft_sdk::api;
use extism::{Manifest, Plugin, Wasm};
use hex::FromHex;
use plugin_api::*;
use pod2::middleware::Hash;

/// Hosts a loaded `.pexe` plugin and provides query + execution APIs.
///
/// In Phase 1 the built-in plugin WASM is embedded via `include_bytes!`.
/// In Phase 2 this will support loading external `.pexe` files at runtime.
pub struct PluginHost {
    metadata: PluginMetadata,
}

/// The built-in minecraft plugin WASM module, compiled from `pexe_minecraft/`.
const BUILTIN_WASM: &[u8] =
    include_bytes!("../../data/plugins/minecraft-basics.pexe");

impl PluginHost {
    /// Load the built-in plugin.
    pub fn builtin() -> Result<Self> {
        Self::from_wasm(BUILTIN_WASM)
    }

    /// Load a plugin from raw WASM bytes.
    pub fn from_wasm(wasm_bytes: &[u8]) -> Result<Self> {
        let manifest = Manifest::new([Wasm::data(wasm_bytes)]);
        let mut plugin = Plugin::new(&manifest, [], true)?;
        let metadata_json = plugin.call::<&[u8], &[u8]>("get_metadata", &[])?;
        let metadata: PluginMetadata = serde_json::from_slice(metadata_json)?;
        Ok(Self { metadata })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_builtin_plugin() {
        let host = PluginHost::builtin().expect("failed to load builtin plugin");
        assert_eq!(host.name(), "minecraft-basics");
    }

    #[test]
    fn test_plugin_classes() {
        let host = PluginHost::builtin().unwrap();
        let classes = host.class_names();
        assert!(classes.contains(&"Log".to_string()));
        assert!(classes.contains(&"Wood".to_string()));
        assert!(classes.contains(&"Stone".to_string()));
        assert!(classes.contains(&"Stick".to_string()));
        assert!(classes.contains(&"WoodPick".to_string()));
        assert!(classes.contains(&"StonePick".to_string()));
        assert_eq!(classes.len(), 6);
    }

    #[test]
    fn test_plugin_actions() {
        let host = PluginHost::builtin().unwrap();
        let descriptors = host.action_descriptors();
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
        let host = PluginHost::builtin().unwrap();
        let visible = host.visible_action_descriptors();
        let hidden_names = ["UseWoodPick", "UseStonePick"];
        for d in &visible {
            assert!(!hidden_names.contains(&d.name.as_str()),
                "hidden action {} should not be visible", d.name);
        }
        assert_eq!(visible.len(), 7);
    }

    #[test]
    fn test_plugin_dependencies() {
        let host = PluginHost::builtin().unwrap();
        let deps = host.dependencies();
        assert_eq!(deps.len(), 2);
    }

    #[test]
    fn test_actions_generate_closures() {
        let host = PluginHost::builtin().unwrap();
        let actions = host.actions();
        assert_eq!(actions.len(), 9);
        // Verify action names match
        assert_eq!(actions[0].name, "FindLog");
        assert_eq!(actions[1].name, "CraftWood");
    }

    #[test]
    fn test_class_ui_meta() {
        let host = PluginHost::builtin().unwrap();
        let wood = host.class_ui_meta("Wood");
        assert_eq!(wood.emoji, "🪵");
        let unknown = host.class_ui_meta("Nonexistent");
        assert_eq!(unknown.emoji, "📦");
    }

    #[test]
    fn test_action_io_signatures() {
        let host = PluginHost::builtin().unwrap();
        let by_name = host.action_descriptors_by_name();

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
}
