//! Driver-internal types. Anything that crosses a process boundary is
//! defined in the `wire-types` crate so dobjd/cli/mcp can share it
//! without dragging in the driver's heavy deps (pod2, plonky2, rocksdb).

use std::path::PathBuf;

use pod2::middleware::Hash;
use serde::{Deserialize, Serialize};
use wire_types::{ExecutionPhase, ObjectStatus, QualifiedName};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriverPaths {
    pub settings_path: PathBuf,
    pub objects_dir: PathBuf,
    pub nullified_objects_dir: PathBuf,
    pub actions_dir: PathBuf,
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
    pub old_root: Hash,
    pub new_root: Hash,
    pub output_files: Vec<String>,
    pub nullified_files: Vec<String>,
    pub relayer_job_id: String,
    pub tx_hash: Option<String>,
    pub block_number: Option<i64>,
}

/// Optional context passed alongside execution progress steps.
#[derive(Debug, Clone, Default)]
pub struct ExecutionStepContext {
    /// The state root hash before this execution (available during Commit phase).
    pub old_root: Option<Hash>,
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
