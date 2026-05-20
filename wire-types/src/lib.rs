//! Pure-data wire types shared across the driver, dobjd, MCP, and CLI.
//!
//! Anything that travels over a process boundary (HTTP body, MCP tool
//! parameter, SSE event, CLI arg) belongs here — but only if it has no
//! logic that depends on ZK proofs or chain state. Keeping this crate
//! dependency-light is the entire point: `cli` and `mcp` should be able
//! to use it without compiling pod2/plonky2/rocksdb.

use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};

#[cfg(feature = "schemars")]
use schemars::JsonSchema;

// ===========================================================================
// Identifiers
// ===========================================================================

/// `QualifiedName` is the canonical handle for both classes and actions.
/// It carries the originating plugin and the bare name as two separate
/// fields so callers can reason about them directly without juggling
/// `(plugin_name, name, id)` triples. The string presentation
/// `<plugin>::<name>` matches podlang's namespaced predicates and is
/// produced by [`QualifiedName::id`] when a single string is needed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct QualifiedName {
    pub plugin_name: String,
    pub name: String,
}

impl QualifiedName {
    pub fn new(plugin_name: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            plugin_name: plugin_name.into(),
            name: name.into(),
        }
    }

    /// Canonical string form `<plugin>::<name>` — matches podlang's
    /// namespaced-predicate syntax and is what the GUI shows the user.
    pub fn id(&self) -> String {
        format!("{}::{}", self.plugin_name, self.name)
    }

    /// Parse the canonical string form. Errors if the input does not
    /// contain `::` or if either component is empty.
    pub fn parse(s: &str) -> Result<Self, String> {
        let (plugin, name) = s
            .split_once("::")
            .ok_or_else(|| format!("invalid qualified name {s:?}: missing '::' separator"))?;
        if plugin.is_empty() {
            return Err(format!("invalid qualified name {s:?}: empty plugin"));
        }
        if name.is_empty() {
            return Err(format!("invalid qualified name {s:?}: empty name"));
        }
        Ok(Self {
            plugin_name: plugin.to_string(),
            name: name.to_string(),
        })
    }

    /// Lowercase, filename-safe prefix for `.dobj` files. Both plugin
    /// names (validated at catalog load) and class names (validated by
    /// the SDK at module compile time) are already restricted to
    /// `[A-Za-z0-9_-]`, so the only normalization this needs to do is
    /// lowercase. The fallback to `_` for any non-allowlisted byte stays
    /// in place as a defense-in-depth measure: if a future SDK regression
    /// or an entirely different catalog implementation ever feeds in a
    /// stray path separator, written files will still be confined to a
    /// single filename component.
    pub fn file_prefix(&self) -> String {
        let mut out = String::with_capacity(self.plugin_name.len() + 2 + self.name.len());
        push_safe_lower(&mut out, &self.plugin_name);
        out.push_str("__");
        push_safe_lower(&mut out, &self.name);
        out
    }
}

impl fmt::Display for QualifiedName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}::{}", self.plugin_name, self.name)
    }
}

fn push_safe_lower(out: &mut String, s: &str) {
    for ch in s.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() || lower == '-' || lower == '_' {
            out.push(lower);
        } else {
            out.push('_');
        }
    }
}

/// One entry in an action's input/output slot list, or a missing-slot
/// entry in a feasibility report. Pairs the class identity with its
/// on-chain `Is{class}` predicate hash.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ClassRef {
    pub class: QualifiedName,
    /// Hex-encoded `Is{class}` predicate hash. Empty if the catalog
    /// could not derive it (shouldn't happen for compiled modules).
    pub hash: String,
}

// ===========================================================================
// Object lifecycle
// ===========================================================================

/// Lifecycle of a Digital Object on this driver. `Live` and `Nullified`
/// both mean the source tx is canonical on-chain; `Pending` means
/// relayer-accepted but not yet observed by the synchronizer; `Unknown`
/// means neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum ObjectStatus {
    Unknown,
    Pending,
    Live,
    Nullified,
}

impl ObjectStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            ObjectStatus::Unknown => "unknown",
            ObjectStatus::Pending => "pending",
            ObjectStatus::Live => "live",
            ObjectStatus::Nullified => "nullified",
        }
    }
}

impl fmt::Display for ObjectStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Summary view of an object on disk. Returned by `/objects/{name}` and
/// surfaced by the driver to every client.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ObjectSummary {
    pub id: String,
    pub file_name: String,
    pub class: QualifiedName,
    pub class_hash: String,
    pub status: ObjectStatus,
    pub tx_hash: Option<String>,
    pub fields: HashMap<String, serde_json::Value>,
}

/// Inventory row served by `GET /inventory`. Folds class metadata (emoji,
/// description) into the object summary so GUI clients can render rows
/// without a second `/classes` round-trip.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct InventoryObject {
    pub id: String,
    pub file_name: String,
    pub class: QualifiedName,
    pub class_hash: String,
    pub emoji: String,
    pub status: ObjectStatus,
    pub tx_hash: Option<String>,
    pub description: Option<String>,
    /// Application-layer fields (e.g. `durability`, `key`, `work`).
    /// Same shape as [`ObjectSummary::fields`].
    pub fields: HashMap<String, serde_json::Value>,
}

// ===========================================================================
// Catalog
// ===========================================================================

/// Summary view of an action declared by a plugin.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ActionSummary {
    pub action: QualifiedName,
    pub emoji: String,
    pub hash: String,
    pub description: String,
    pub total_inputs: Vec<ClassRef>,
    pub total_outputs: Vec<ClassRef>,
    /// Podlang source for this action's predicate. Empty if the catalog
    /// can't locate it (shouldn't happen for compiled plugins).
    pub predicate_source: String,
}

/// Summary view of a class declared by a plugin.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
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

// ===========================================================================
// Action execution
// ===========================================================================

/// One candidate input for a feasibility check.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct CheckActionCandidate {
    pub class: QualifiedName,
    pub object_id: String,
    pub file_name: String,
}

/// Result of `Driver::check_action` / `GET /actions/{plugin}/{name}/feasibility`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct CheckActionReport {
    pub feasible: bool,
    pub action: QualifiedName,
    pub available_inputs: Vec<CheckActionCandidate>,
    /// Slots that had no eligible live object in inventory.
    pub missing_inputs: Vec<ClassRef>,
}

/// `POST /actions/run` body.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct RunActionInput {
    pub action: QualifiedName,
    pub input_object_paths: Vec<String>,
    /// Client-generated correlation id for filtering progress events on
    /// `/events`. Optional on the wire so clients that don't care about
    /// correlation can omit it — dobjd mints one if missing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
}

/// Wrapper to keep parity with the legacy Tauri command shape
/// (`{ "input": { ... } }`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct RunActionRequest {
    pub input: RunActionInput,
}

/// `POST /actions/run` response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct RunActionResult {
    /// Correlation id used for `run-action-progress` events. Echoed
    /// from the request when supplied, otherwise a freshly-minted UUID v4.
    pub run_id: String,
    pub old_root: String,
    pub new_root: String,
    pub output_files: Vec<String>,
    pub nullified_files: Vec<String>,
}

// ===========================================================================
// Execution progress
// ===========================================================================

/// Which phase of action execution is in progress. `GenerateProof` covers
/// the local ZK proof construction; `Commit` covers payload submission to
/// the relayer plus filesystem writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum ExecutionPhase {
    GenerateProof,
    Commit,
}

/// Step status carried alongside a phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum ProofProgressStatus {
    Running,
    Done,
    Failed,
}

/// Wire shape for a single `run-action-progress` SSE event. Subscribers
/// filter by `run_id` to scope events to their own action.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct RunActionProgress {
    pub run_id: String,
    pub phase: ExecutionPhase,
    pub status: ProofProgressStatus,
    pub message: String,
    pub old_root: Option<String>,
    pub new_root: Option<String>,
    pub output_files: Option<Vec<String>>,
    pub output_status: Option<ObjectStatus>,
    pub nullified_files: Option<Vec<String>>,
}

// ===========================================================================
// Driver configuration
// ===========================================================================

/// The driver's persisted configuration: synchronizer + relayer URLs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct DriverSettings {
    pub synchronizer_api_url: String,
    pub relayer_api_url: String,
}

/// Filesystem location of the local objects directory. Returned by
/// `GET /objects/dir` and the MCP `get_objects_dir` tool.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ObjectsDirInfo {
    /// Absolute path to `~/.dobj/objects/`.
    pub path: String,
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qualified_name_id_round_trips_via_parse() {
        let q = QualifiedName::new("craft-basics", "WoodPick");
        assert_eq!(q.id(), "craft-basics::WoodPick");
        assert_eq!(QualifiedName::parse(&q.id()).unwrap(), q);
    }

    #[test]
    fn qualified_name_parse_rejects_missing_separator() {
        assert!(QualifiedName::parse("craft-basics-Wood").is_err());
    }

    #[test]
    fn qualified_name_parse_rejects_empty_components() {
        assert!(QualifiedName::parse("::Wood").is_err());
        assert!(QualifiedName::parse("craft-basics::").is_err());
    }

    #[test]
    fn qualified_name_file_prefix_is_path_safe_for_normal_names() {
        let q = QualifiedName::new("craft-basics", "Wood");
        assert_eq!(q.file_prefix(), "craft-basics__wood");
    }

    #[test]
    fn qualified_name_file_prefix_sanitizes_path_chars_in_name() {
        let q = QualifiedName::new("plugin", "weird/class");
        assert_eq!(q.file_prefix(), "plugin__weird_class");
        let q = QualifiedName::new("plugin", "..\\Stone");
        assert_eq!(q.file_prefix(), "plugin_____stone");
        let q = QualifiedName::new("p", "c");
        let p = q.file_prefix();
        assert!(!p.contains(':') && !p.contains('/') && !p.contains('\\'));
    }

    #[test]
    fn qualified_name_serde_round_trips_as_object() {
        let q = QualifiedName::new("craft-basics", "Wood");
        let json = serde_json::to_string(&q).unwrap();
        assert_eq!(json, r#"{"pluginName":"craft-basics","name":"Wood"}"#);
        let back: QualifiedName = serde_json::from_str(&json).unwrap();
        assert_eq!(back, q);
    }

    #[test]
    fn object_status_serializes_lowercase() {
        assert_eq!(
            serde_json::to_string(&ObjectStatus::Live).unwrap(),
            "\"live\""
        );
        assert_eq!(ObjectStatus::Pending.to_string(), "pending");
    }

    #[test]
    fn run_action_input_serializes_without_run_id_when_none() {
        let input = RunActionInput {
            action: QualifiedName::new("craft-basics", "FindLog"),
            input_object_paths: vec![],
            run_id: None,
        };
        let json = serde_json::to_string(&input).unwrap();
        assert!(!json.contains("runId"), "got: {json}");
    }
}
