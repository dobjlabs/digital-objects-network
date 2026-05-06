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
    /// Qualified class id, e.g. "craft-basics::WoodPick"
    pub id: String,
    /// Bare class name, e.g. "WoodPick"
    pub display_name: String,
    /// Plugin that declares this class
    pub plugin_name: String,
    /// Number of live objects of this class in inventory
    pub live_count: usize,
    /// Qualified action ids that produce objects of this class
    pub produced_by: Vec<String>,
    /// Qualified action ids that consume objects of this class
    pub consumed_by: Vec<String>,
}

// -- Inventory / State --

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InventoryObject {
    /// Unique object identifier (hex hash)
    pub id: String,
    /// Qualified class id, e.g. "craft-basics::WoodPick"
    pub class_id: String,
    /// Bare class name, e.g. "WoodPick"
    pub class_display_name: String,
    /// Plugin that declares this object's class
    pub plugin_name: String,
    /// The .dobj filename
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
pub struct ClassRef {
    /// Qualified class id, e.g. "craft-basics::WoodPick"
    pub id: String,
    /// Bare class name, e.g. "WoodPick"
    pub display_name: String,
    /// Hex-encoded `Is{class}` predicate hash
    pub hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Action {
    /// Qualified action id, e.g. "craft-basics::CraftWoodPick"
    pub id: String,
    /// Bare action name, e.g. "CraftWoodPick"
    pub display_name: String,
    /// Plugin that provides this action
    pub plugin_name: String,
    /// Human-readable description
    pub description: String,
    /// Classes required as inputs
    pub total_inputs: Vec<ClassRef>,
    /// Classes produced as outputs
    pub total_outputs: Vec<ClassRef>,
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
    /// Qualified class id, e.g. "craft-basics::WoodPick"
    pub class_id: String,
    /// Bare class name, e.g. "WoodPick"
    pub class_display_name: String,
    /// Plugin that declares this object's class
    pub plugin_name: String,
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
    /// Qualified class id, e.g. "craft-basics::WoodPick"
    pub class_id: String,
    /// Bare class name
    pub class_display_name: String,
    /// Plugin that declares this class
    pub plugin_name: String,
    /// Predicate definition in podlang source
    pub predicate_source: String,
    /// Qualified action ids that produce objects of this class
    pub produced_by: Vec<String>,
    /// Qualified action ids that consume objects of this class
    pub consumed_by: Vec<String>,
}

// -- Actions --

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunActionInput {
    /// The qualified action id to execute, e.g. "craft-basics::CraftWoodPick"
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
    /// The qualified action id being checked
    pub action_id: String,
    /// Objects available to satisfy input requirements
    pub available_inputs: Vec<FeasibilityInput>,
    /// Slots that had no eligible object in inventory
    pub missing_inputs: Vec<ClassRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FeasibilityInput {
    /// Qualified class id of the available object
    pub class_id: String,
    /// Bare class name
    pub class_display_name: String,
    /// Plugin that declares the class
    pub plugin_name: String,
    /// Object identifier
    pub object_id: String,
    /// The .dobj filename
    pub file_name: String,
}
