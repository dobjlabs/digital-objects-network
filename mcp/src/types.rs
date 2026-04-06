use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// -- List response wrappers (MCP outputSchema requires root type "object") --

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InventoryList {
    /// All objects in the inventory
    pub objects: Vec<InventoryObject>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActionList {
    /// All available crafting actions
    pub actions: Vec<Action>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClassList {
    /// All known object classes
    pub classes: Vec<ClassSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClassSummary {
    /// Class name, e.g. "WoodPick"
    pub name: String,
    /// Number of live objects of this class in inventory
    pub live_count: usize,
    /// Actions that produce objects of this class
    pub produced_by: Vec<String>,
    /// Actions that consume objects of this class
    pub consumed_by: Vec<String>,
}

// -- Inventory / State --

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InventoryObject {
    /// Unique object identifier (hex hash)
    pub id: String,
    /// Object class name, e.g. "WoodPick"
    pub class_name: String,
    /// The .dobj filename, e.g. "WoodPick.dobj"
    pub file_name: String,
    /// Lifecycle status: "unknown", "pending", "live", or "nullified"
    pub status: String,
    /// Transaction commitment hash (hex) if this object has been submitted
    pub tx_hash: Option<String>,
    /// Application-layer fields as key-value pairs
    pub fields: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Action {
    /// Action identifier, e.g. "CraftWoodPick"
    pub id: String,
    /// Human-readable description
    pub description: String,
    /// Class names required as inputs
    pub input_classes: Vec<String>,
    /// Class names produced as outputs
    pub output_classes: Vec<String>,
    /// Approximate CPU cost, e.g. "~10s"
    pub cpu_cost: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StateRootResponse {
    /// The current global state root hash
    pub state_root: String,
}

// -- Object inspection --

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ObjectDetail {
    /// Unique object identifier (hex hash)
    pub id: String,
    /// Object class name
    pub class_name: String,
    /// Lifecycle status: "unknown", "pending", "live", or "nullified"
    pub status: String,
    /// Transaction commitment hash (hex) if this object has been submitted
    pub tx_hash: Option<String>,
    /// Application-layer state fields
    pub state: HashMap<String, serde_json::Value>,
    /// Podlang predicate source for this object's class
    pub predicate_source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClassDetail {
    /// Class name
    pub class_name: String,
    /// Predicate definition in podlang source
    pub predicate_source: String,
    /// Actions that produce objects of this class
    pub produced_by: Vec<String>,
    /// Actions that consume objects of this class
    pub consumed_by: Vec<String>,
}

// -- Actions --

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunActionInput {
    /// The action to execute, e.g. "CraftWoodPick"
    pub action_id: String,
    /// Paths to .dobj files to use as inputs
    pub input_object_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunActionResult {
    /// Whether the action succeeded
    pub success: bool,
    /// Human-readable status message
    pub message: String,
    /// Objects produced by the action
    pub outputs: Vec<InventoryObject>,
    /// Objects consumed (nullified) by the action
    pub consumed: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FeasibilityReport {
    /// Whether the action can be executed with current inventory
    pub feasible: bool,
    /// The action being checked
    pub action_id: String,
    /// Objects available to satisfy input requirements
    pub available_inputs: Vec<FeasibilityInput>,
    /// Input classes that have no matching object in inventory
    pub missing_inputs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FeasibilityInput {
    /// Class name of the available object
    pub class_name: String,
    /// Object identifier
    pub object_id: String,
    /// The .dobj filename
    pub file_name: String,
}
