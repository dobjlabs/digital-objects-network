//! Thin adapter that delegates all spec queries to the plugin orchestrator.
//!
//! This module preserves the same `pub(crate)` API surface that the rest of
//! the app-gui crate expects (`actions()`, `dependencies()`, etc.), but the
//! actual definitions now live in per-action `.rhai` scripts loaded by the
//! `PluginOrchestrator`, each compiled to its own pod2 module.

use std::collections::HashMap;
use std::sync::LazyLock;

use craft_sdk::ActionGroup;
use plugin_host::PluginOrchestrator;

// Re-export the shared types so existing call sites keep compiling.
pub(crate) use plugin_api::{ActionDescriptor, ClassUiMeta};

/// The singleton plugin orchestrator, initialized once with the built-in action scripts.
static ORCH: LazyLock<PluginOrchestrator> = LazyLock::new(|| {
    PluginOrchestrator::builtin().expect("failed to load built-in action plugins")
});

// ---------------------------------------------------------------------------
// Public (crate) API — matches the original spec.rs signatures exactly.
// ---------------------------------------------------------------------------

/// Build action groups for `Helper::new_multi_module()`.
pub(crate) fn action_groups() -> Vec<ActionGroup> {
    ORCH.action_groups()
}

/// All action descriptors (including hidden/internal).
#[allow(dead_code)]
pub(crate) fn action_descriptors() -> Vec<ActionDescriptor> {
    ORCH.action_descriptors()
}

/// Only user-facing (non-hidden) action descriptors.
pub(crate) fn visible_action_descriptors() -> Vec<ActionDescriptor> {
    ORCH.visible_action_descriptors()
}

/// Action descriptors indexed by name.
pub(crate) fn action_descriptors_by_name() -> HashMap<String, ActionDescriptor> {
    ORCH.action_descriptors_by_name()
}

/// All unique class names (sorted).
pub(crate) fn class_names() -> Vec<String> {
    ORCH.class_names()
}

/// Look up UI metadata for a class by name.
pub(crate) fn class_ui_meta(class_name: &str) -> ClassUiMeta {
    ORCH.class_ui_meta(class_name)
}
