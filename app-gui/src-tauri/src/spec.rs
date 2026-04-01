//! Thin adapter that delegates all spec queries to the plugin host.
//!
//! This module preserves the same `pub(crate)` API surface that the rest of
//! the app-gui crate expects (`actions()`, `dependencies()`, etc.), but the
//! actual definitions now live in a `.pexe` Rhai script loaded by `plugin_host`.

use std::collections::HashMap;
use std::sync::LazyLock;

use craft_sdk::api;
use plugin_host::PluginHost;

// Re-export the shared types so existing call sites keep compiling.
pub(crate) use plugin_api::{ActionDescriptor, ClassUiMeta};

/// The singleton plugin host, initialized once with the built-in minecraft plugin.
static HOST: LazyLock<PluginHost> = LazyLock::new(|| {
    PluginHost::builtin().expect("failed to load built-in pexe plugin")
});

// ---------------------------------------------------------------------------
// Public (crate) API — matches the original spec.rs signatures exactly.
// ---------------------------------------------------------------------------

/// Fresh action definitions with closures (must be called per proof-generation).
pub(crate) fn actions() -> Vec<api::Action> {
    HOST.actions()
}

/// Intro-pod and module dependency declarations.
pub(crate) fn dependencies() -> Vec<api::Dependency> {
    HOST.dependencies()
}

/// All action descriptors (including hidden/internal).
#[allow(dead_code)]
pub(crate) fn action_descriptors() -> Vec<ActionDescriptor> {
    HOST.action_descriptors()
}

/// Only user-facing (non-hidden) action descriptors.
pub(crate) fn visible_action_descriptors() -> Vec<ActionDescriptor> {
    HOST.visible_action_descriptors()
}

/// Action descriptors indexed by name.
pub(crate) fn action_descriptors_by_name() -> HashMap<String, ActionDescriptor> {
    HOST.action_descriptors_by_name()
}

/// All unique class names (sorted).
pub(crate) fn class_names() -> Vec<String> {
    HOST.class_names()
}

/// Look up UI metadata for a class by name.
pub(crate) fn class_ui_meta(class_name: &str) -> ClassUiMeta {
    HOST.class_ui_meta(class_name)
}
