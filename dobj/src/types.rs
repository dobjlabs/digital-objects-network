//! Wire types — kept in sync with dobjd's route handlers.
//!
//! These are minimal parsing types: the CLI never needs the full structure
//! of e.g. an `ObjectRecord`'s embedded pod proof, only the human-readable
//! fields. Anything beyond that comes through as `serde_json::Value`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InventoryObject {
    pub id: String,
    pub file_name: String,
    pub class_name: String,
    pub status: String,
    pub tx_hash: Option<String>,
    pub grounded: bool,
    pub emoji: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionSummary {
    pub id: String,
    pub emoji: String,
    pub description: String,
    pub total_input_classes: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadGuiInventoryResult {
    pub inventory: Vec<InventoryObject>,
    pub actions: Vec<ActionSummary>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunActionInput {
    pub action_id: String,
    pub input_object_paths: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct RunActionRequest {
    pub input: RunActionInput,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunActionResult {
    pub ok: bool,
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
