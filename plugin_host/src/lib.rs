use std::collections::HashMap;

use craft_sdk::api;
use plugin_api::{
    ActionDescriptor, ClassUiMeta, PluginSpec,
    action_descriptors, action_descriptors_by_name, class_names, class_ui_meta,
    visible_action_descriptors,
};

/// Hosts one or more loaded plugins and provides a unified query API.
///
/// In Phase 1 the host holds a single statically-linked plugin.
/// In Phase 2 this will be extended to load WASM modules at runtime.
pub struct PluginHost {
    plugin: Box<dyn PluginSpec + Send + Sync>,
}

impl PluginHost {
    /// Create a host from a statically-linked plugin.
    pub fn from_builtin(plugin: impl PluginSpec + Send + Sync + 'static) -> Self {
        Self {
            plugin: Box::new(plugin),
        }
    }

    /// Plugin name.
    pub fn name(&self) -> &'static str {
        self.plugin.name()
    }

    /// Dependencies for proof generation (intro pods, modules).
    pub fn dependencies(&self) -> Vec<api::Dependency> {
        self.plugin.dependencies()
    }

    /// Fresh action definitions with closures for proof generation.
    /// Must be called fresh each time (closures are not Clone).
    pub fn actions(&self) -> Vec<api::Action> {
        self.plugin.actions()
    }

    /// All action descriptors (including hidden ones).
    pub fn action_descriptors(&self) -> Vec<ActionDescriptor> {
        action_descriptors(&*self.plugin)
    }

    /// Only non-hidden action descriptors.
    pub fn visible_action_descriptors(&self) -> Vec<ActionDescriptor> {
        visible_action_descriptors(&*self.plugin)
    }

    /// Action descriptors indexed by name.
    pub fn action_descriptors_by_name(&self) -> HashMap<String, ActionDescriptor> {
        action_descriptors_by_name(&*self.plugin)
    }

    /// All known class names (sorted).
    pub fn class_names(&self) -> Vec<String> {
        class_names(&*self.plugin)
    }

    /// Look up class UI metadata by name.
    pub fn class_ui_meta(&self, class_name: &str) -> ClassUiMeta {
        class_ui_meta(&*self.plugin, class_name)
    }
}
