//! MCP wire types. Everything that overlaps with the HTTP wire format
//! is re-exported from `wire-types` — the MCP server speaks the same
//! shapes as dobjd. The only MCP-specific types here are:
//!
//! - List wrappers (`InventoryList`, `ActionList`, `ClassList`): the MCP
//!   spec requires every tool's `outputSchema` to have root type `object`,
//!   so we can't return bare arrays.
//! - `StateRootResponse`: a one-field wrapper for the same reason.
//! - `RunActionResult`: wraps the wire-types result with two MCP-specific
//!   convenience fields (`success`, `message`) for the agent.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use wire_types::{
    ActionSummary as Action, ActionSummary as ActionDetail,
    CheckActionCandidate as FeasibilityInput, CheckActionReport as FeasibilityReport, ClassRef,
    ClassSummary, ClassSummary as ClassDetail, DriverSettings, InventoryObject, ObjectStatus,
    ObjectSummary as ObjectDetail, ObjectsDirInfo, QualifiedName, RunActionInput,
    RunActionResult as RunActionInner,
};

// -- List response wrappers (MCP outputSchema requires root type "object") --

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InventoryList {
    /// All objects in the inventory.
    pub objects: Vec<InventoryObject>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActionList {
    /// All available actions.
    pub actions: Vec<Action>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClassList {
    /// All known object classes.
    pub classes: Vec<ClassSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StateRootResponse {
    /// The current global state root hash.
    pub state_root: String,
}

// -- run_action result --

/// `run_action` tool output. Wraps the wire-types `RunActionResult` with
/// two agent-facing convenience fields (`success`, `message`); the rest
/// of the shape (run id, roots, output file names, nullified files) is
/// the same as `POST /actions/run`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunActionResult {
    /// Whether the action succeeded.
    pub success: bool,
    /// Human-readable status message.
    pub message: String,
    /// The wire-types run result — runId, oldRoot, newRoot, outputFiles,
    /// nullifiedFiles. Nested so the wrapper stays explicit; agents
    /// navigate to `result.outputFiles` etc.
    pub result: RunActionInner,
}
