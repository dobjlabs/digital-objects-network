//! Wire types — kept in sync with dobjd's route handlers.
//!
//! These are minimal parsing types: the CLI never needs the full structure
//! of e.g. an `ObjectRecord`'s embedded pod proof, only the human-readable
//! fields. Anything beyond that comes through as `serde_json::Value`.

use serde::{Deserialize, Serialize};

/// Plugin-scoped name. Mirrors `driver::QualifiedName` over the wire —
/// the canonical printable form is `<pluginName>::<name>`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct QualifiedName {
    pub plugin_name: String,
    pub name: String,
}

impl std::fmt::Display for QualifiedName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}::{}", self.plugin_name, self.name)
    }
}

/// One entry in an action's input/output slot list. Pairs the class
/// identity with its on-chain `Is{class}` predicate hash.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassRef {
    pub class: QualifiedName,
    /// Read off the wire but not displayed by the CLI today; kept here
    /// for shape parity with `driver::ClassRef`.
    #[allow(dead_code)]
    pub hash: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InventoryObject {
    pub id: String,
    pub file_name: String,
    pub class: QualifiedName,
    pub status: String,
    pub tx_hash: Option<String>,
    pub emoji: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionSummary {
    pub action: QualifiedName,
    pub emoji: String,
    pub hash: String,
    pub description: String,
    pub total_inputs: Vec<ClassRef>,
    pub total_outputs: Vec<ClassRef>,
    pub predicate_source: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunActionInput {
    pub action: QualifiedName,
    pub input_object_paths: Vec<String>,
    /// Client-generated correlation id; the daemon echoes it back in
    /// `run-action-progress` events so we can filter to our own run.
    pub run_id: String,
}

#[derive(Debug, Serialize)]
pub struct RunActionRequest {
    pub input: RunActionInput,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunActionResult {
    pub run_id: String,
    pub old_root: String,
    pub new_root: String,
    pub output_files: Vec<String>,
    pub nullified_files: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub synchronizer_api_url: String,
    pub relayer_api_url: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectsDir {
    pub path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectSummary {
    pub id: String,
    pub file_name: String,
    pub class: QualifiedName,
    pub status: String,
    pub tx_hash: Option<String>,
    pub fields: serde_json::Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassSummary {
    pub class: QualifiedName,
    pub emoji: String,
    pub hash: String,
    pub description: String,
    pub live_count: usize,
    pub produced_by: Vec<QualifiedName>,
    pub consumed_by: Vec<QualifiedName>,
    pub predicate_source: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckActionCandidate {
    pub class: QualifiedName,
    pub object_id: String,
    pub file_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckActionReport {
    pub feasible: bool,
    pub action: QualifiedName,
    pub available_inputs: Vec<CheckActionCandidate>,
    pub missing_inputs: Vec<ClassRef>,
}
