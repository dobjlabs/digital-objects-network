//! Driver-internal types. Anything that crosses a process boundary is
//! defined in the `wire-types` crate so dobjd/cli/mcp can share it
//! without dragging in the driver's heavy deps (pod2, plonky2, rocksdb).

use std::path::PathBuf;

use pod2::middleware::Hash;
use wire_types::{ActionArgValues, ExecutionPhase, ExecutionStage, ObjectStatus, QualifiedName};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriverPaths {
    pub settings_path: PathBuf,
    pub imported_keys_path: PathBuf,
    pub objects_dir: PathBuf,
    pub nullified_objects_dir: PathBuf,
    pub discarded_objects_dir: PathBuf,
    pub actions_dir: PathBuf,
    pub transactions_dir: PathBuf,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObjectQuery {
    pub class: Option<QualifiedName>,
    pub status: Option<ObjectStatus>,
    pub exclude_nullified: bool,
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
    /// Overrides by name in pod2 JSON format. Omitted arguments use defaults.
    pub args: ActionArgValues,
}

/// Optional context passed alongside execution progress steps.
#[derive(Debug, Clone, Default)]
pub struct ExecutionStepContext {
    /// Measured substage, independent of the broader proof/commit phase.
    pub stage: Option<ExecutionStage>,
    /// The state root hash before this execution (available during Commit phase).
    pub old_root: Option<Hash>,
    /// Output files touched by this step, when the step corresponds to a
    /// filesystem write. Surfaced to clients so a GUI can light up rows as
    /// they appear / change status.
    pub output_files: Vec<String>,
    /// Shared status written to all `output_files` for this step.
    pub output_status: Option<ObjectStatus>,
}

/// Cloned into SDK callbacks that outlive the borrowed execution context.
pub trait ExecutionReporter: Clone + Send + Sync + 'static {
    /// Called after acquiring inputs, before network requests or proving.
    fn on_inputs_held(&self, _files: &[String]) {}
    fn on_step(&self, _phase: ExecutionPhase, _message: &str, _ctx: &ExecutionStepContext) {}
    fn on_done(&self, _phase: ExecutionPhase) {}
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NoopExecutionReporter;

impl ExecutionReporter for NoopExecutionReporter {}
