use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::object_record::ObjectStatus;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriverPaths {
    pub settings_path: PathBuf,
    pub objects_dir: PathBuf,
    pub nullified_objects_dir: PathBuf,
    pub actions_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DriverSettings {
    pub synchronizer_api_url: String,
    pub relayer_api_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectSelector {
    FileName(String),
    ObjectId(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObjectQuery {
    /// Qualified class id (`<plugin>:<class>`).
    pub class_id: Option<String>,
    pub status: Option<ObjectStatus>,
    pub id: Option<String>,
    pub file_name: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActionQuery {
    /// Qualified action id (`<plugin>:<action>`).
    pub id: Option<String>,
    /// Qualified class id (`<plugin>:<class>`).
    pub input_class_id: Option<String>,
    /// Qualified class id (`<plugin>:<class>`).
    pub output_class_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ObjectSummary {
    pub id: String,
    pub file_name: String,
    /// Qualified class id (`<plugin>:<class>`).
    pub class_id: String,
    /// Bare class name from the producing plugin's manifest.
    pub class_display_name: String,
    pub plugin_name: String,
    pub class_hash: String,
    pub status: ObjectStatus,
    pub tx_hash: Option<String>,
    pub grounded: Option<bool>,
    pub fields: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ActionSummary {
    /// Qualified action id (`<plugin>:<action>`).
    pub id: String,
    pub display_name: String,
    pub plugin_name: String,
    pub emoji: String,
    pub hash: String,
    pub description: String,
    pub total_input_class_ids: Vec<String>,
    pub total_input_class_names: Vec<String>,
    pub total_input_class_hashes: Vec<String>,
    pub total_output_class_ids: Vec<String>,
    pub total_output_class_names: Vec<String>,
    pub total_output_class_hashes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClassSummary {
    /// Qualified class id (`<plugin>:<class>`).
    pub id: String,
    pub display_name: String,
    pub plugin_name: String,
    pub emoji: String,
    pub hash: String,
    pub description: String,
    pub live_count: usize,
    pub produced_by: Vec<String>,
    pub consumed_by: Vec<String>,
    pub predicate_source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CheckActionCandidate {
    pub class_id: String,
    pub class_display_name: String,
    pub plugin_name: String,
    pub object_id: String,
    pub file_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CheckActionReport {
    pub feasible: bool,
    /// Qualified action id (`<plugin>:<action>`).
    pub action_id: String,
    pub available_inputs: Vec<CheckActionCandidate>,
    /// Qualified class ids that have no live object in inventory.
    pub missing_input_class_ids: Vec<String>,
    /// Bare class names parallel to `missing_input_class_ids`.
    pub missing_input_class_names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecuteActionInput {
    pub action_id: String,
    pub input_objects: Vec<ObjectSelector>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExecuteActionResult {
    pub old_root: String,
    pub new_root: String,
    pub output_files: Vec<String>,
    pub nullified_files: Vec<String>,
    pub relayer_job_id: String,
    pub tx_hash: Option<String>,
    pub block_number: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionPhase {
    GenerateProof,
    Commit,
}

/// Optional context passed alongside execution progress steps.
#[derive(Debug, Clone, Default)]
pub struct ExecutionStepContext {
    /// The state root hash before this execution (available during Commit phase).
    pub old_root: Option<String>,
}

pub trait ExecutionReporter {
    fn on_step(&self, _phase: ExecutionPhase, _message: &str, _ctx: &ExecutionStepContext) {}
    fn on_done(&self, _phase: ExecutionPhase, _result: Option<&ExecuteActionResult>) {}
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NoopExecutionReporter;

impl ExecutionReporter for NoopExecutionReporter {}
