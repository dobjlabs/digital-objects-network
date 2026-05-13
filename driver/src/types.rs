use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use wire_types::ClassRef;

use crate::object_record::ObjectStatus;
use wire_types::QualifiedName;

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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObjectQuery {
    pub class: Option<QualifiedName>,
    pub status: Option<ObjectStatus>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActionQuery {
    pub action: Option<QualifiedName>,
    pub input_class: Option<QualifiedName>,
    pub output_class: Option<QualifiedName>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ObjectSummary {
    pub id: String,
    pub file_name: String,
    pub class: QualifiedName,
    pub class_hash: String,
    /// Lifecycle. `Live` and `Nullified` both mean the source tx is
    /// canonical on-chain; `Pending` means relayer-accepted but not yet
    /// observed by the synchronizer; `Unknown` means neither.
    pub status: ObjectStatus,
    pub tx_hash: Option<String>,
    pub fields: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ActionSummary {
    pub action: QualifiedName,
    pub emoji: String,
    pub hash: String,
    pub description: String,
    pub total_inputs: Vec<ClassRef>,
    pub total_outputs: Vec<ClassRef>,
    /// Podlang source for this action's predicate, extracted from the
    /// generated podlang module. Empty if the catalog can't locate it
    /// (shouldn't happen for compiled plugins).
    pub predicate_source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CheckActionCandidate {
    pub class: QualifiedName,
    pub object_id: String,
    pub file_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CheckActionReport {
    pub feasible: bool,
    pub action: QualifiedName,
    pub available_inputs: Vec<CheckActionCandidate>,
    /// Slots that had no eligible live object in inventory.
    pub missing_inputs: Vec<ClassRef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecuteActionInput {
    pub action: QualifiedName,
    /// `.dobj` files this action should consume, ordered to match the
    /// action's input class slots. Each entry can be a bare basename
    /// (`Wood.dobj`) or a longer path — only the file name is used, and
    /// it must resolve to a live object inside `~/.dobj/objects/`.
    pub input_objects: Vec<String>,
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
    /// Output files touched by this step, when the step corresponds to a
    /// filesystem write. Surfaced to clients so a GUI can light up rows as
    /// they appear / change status.
    pub output_files: Vec<String>,
    /// Shared status written to all `output_files` for this step.
    pub output_status: Option<ObjectStatus>,
}

pub trait ExecutionReporter {
    fn on_step(&self, _phase: ExecutionPhase, _message: &str, _ctx: &ExecutionStepContext) {}
    fn on_done(&self, _phase: ExecutionPhase, _result: Option<&ExecuteActionResult>) {}
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NoopExecutionReporter;

impl ExecutionReporter for NoopExecutionReporter {}
