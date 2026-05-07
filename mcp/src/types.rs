use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A name scoped to a plugin. Both classes and actions are identified by a
/// `(plugin_name, name)` pair; the printable form `<plugin>::<name>` matches
/// podlang's namespaced-predicate syntax. The MCP crate carries its own
/// mirror of `driver::QualifiedName` so the MCP boundary doesn't pull in
/// the rest of the driver crate.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct QualifiedName {
    /// Originating plugin, e.g. "craft-basics".
    pub plugin_name: String,
    /// Bare class or action name, e.g. "WoodPick".
    pub name: String,
}

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
    /// Plugin-scoped class identity.
    pub class: QualifiedName,
    /// Number of live objects of this class in inventory
    pub live_count: usize,
    /// Actions that produce objects of this class
    pub produced_by: Vec<QualifiedName>,
    /// Actions that consume objects of this class
    pub consumed_by: Vec<QualifiedName>,
}

// -- Inventory / State --

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InventoryObject {
    /// Unique object identifier (hex hash)
    pub id: String,
    /// Plugin-scoped class identity.
    pub class: QualifiedName,
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
    /// Plugin-scoped class identity.
    pub class: QualifiedName,
    /// Hex-encoded `Is{class}` predicate hash
    pub hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Action {
    /// Plugin-scoped action identity.
    pub action: QualifiedName,
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
    /// Plugin-scoped class identity.
    pub class: QualifiedName,
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
    /// Plugin-scoped class identity.
    pub class: QualifiedName,
    /// Predicate definition in podlang source
    pub predicate_source: String,
    /// Actions that produce objects of this class
    pub produced_by: Vec<QualifiedName>,
    /// Actions that consume objects of this class
    pub consumed_by: Vec<QualifiedName>,
}

// -- Actions --

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunActionInput {
    /// Plugin-scoped action to execute, e.g. `{pluginName: "craft-basics", name: "CraftWoodPick"}`.
    pub action: QualifiedName,
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
    pub action: QualifiedName,
    /// Objects available to satisfy input requirements
    pub available_inputs: Vec<FeasibilityInput>,
    /// Slots that had no eligible object in inventory
    pub missing_inputs: Vec<ClassRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FeasibilityInput {
    /// Plugin-scoped class identity of the available object.
    pub class: QualifiedName,
    /// Object identifier
    pub object_id: String,
    /// The .dobj filename
    pub file_name: String,
}
