//! Pure-data wire types shared across the driver, dobjd, MCP, and CLI.
//!
//! Anything that travels over a process boundary (HTTP body, MCP tool
//! parameter, SSE event, CLI arg) belongs here — but only if it has no
//! logic that depends on ZK proofs or chain state. Keeping this crate
//! dependency-light is the entire point: `cli` and `mcp` should be able
//! to use it without compiling pod2/plonky2/rocksdb/sqlx.
//!
//! ## Submodules
//!
//! - [`relayer`] — DTOs for the relayer's HTTP API. The status/request
//!   types are pure serde; the two proof-bearing response types carry pod2
//!   `Hash` (`tx_final`, `state_root`), so they live behind the
//!   `chain` feature. The relayer server and the driver enable it.
//! - [`synchronizer`] — DTOs for the synchronizer's HTTP API. Every type
//!   carries pod2 `Hash` values directly, so the whole module lives behind
//!   the `chain` feature. Only the synchronizer server and the driver speak
//!   this API, and both enable the feature; `cli` and `mcp` don't.

pub mod relayer;
#[cfg(feature = "chain")]
pub mod synchronizer;

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

/// Summary view of an object on disk: the object's own fields plus its
/// class's display metadata (emoji, description), folded in so clients can
/// render rows without a second `/classes` round-trip. Surfaced by the
/// driver to every client and returned by `GET /objects` and
/// `GET /objects/{name}`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ObjectSummary {
    pub content_hash: String,
    pub file_name: String,
    pub class: QualifiedName,
    pub class_hash: String,
    /// The object's class display emoji (`📦` when the class is unknown).
    pub emoji: String,
    pub status: ObjectStatus,
    pub tx_hash: Option<String>,
    /// The object's class description, when its class is known.
    pub description: Option<String>,
    /// Application-layer fields (e.g. `durability`, `key`, `work`).
    pub fields: HashMap<String, serde_json::Value>,
}

/// `POST /objects/import` body — the raw JSON contents of an external `.dobj`
/// file, one not produced by this driver (e.g. from outside `~/.dobj/`). The driver
/// validates the object's class identity and on-chain grounding, then files
/// it into the local object store under a canonical name derived from its
/// commitment. Consumers handle their own file I/O and pass the bytes as a
/// string. The import result is a plain [`ObjectSummary`] whose `status`
/// reflects grounding (`live` if the source tx is canonical, else `unknown`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ImportObjectRequest {
    pub dobj: String,
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
    pub content_hash: String,
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
    /// Slots that had no eligible live object in the local objects.
    pub missing_inputs: Vec<ClassRef>,
}

/// `POST /actions/run` body.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct RunActionInput {
    pub action: QualifiedName,
    pub input_object_paths: Vec<String>,
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
    /// Daemon-assigned id for this run (also the key for
    /// `GET /actions/runs/{run_id}` and the scope of its `run-action-progress`
    /// events).
    pub run_id: String,
    pub old_root: String,
    pub new_root: String,
    pub output_files: Vec<String>,
    pub nullified_files: Vec<String>,
}

/// Lifecycle state of a run tracked in the daemon's run registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum RunStatus {
    /// Accepted, not yet started executing.
    Queued,
    /// Building the local ZK proof.
    GenerateProof,
    /// Submitting the proof, then waiting for on-chain confirmation + sync.
    Committing,
    /// Finished successfully; `RunState::result` is populated.
    Succeeded,
    /// Finished with an error; `RunState::error` is populated.
    Failed,
}

/// `POST /actions/run` response. The run was accepted and now executes in the
/// background; follow it via `GET /actions/runs/{runId}` (poll) or
/// `GET /actions/runs/{runId}/events` (SSE, replayable via `Last-Event-ID`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct RunAccepted {
    pub run_id: String,
    pub status: RunStatus,
}

/// `GET /actions/runs/{runId}` response: the current state of a run. The
/// poll-and-recover counterpart to the SSE stream — a client that lost its
/// connection re-reads the outcome here for as long as the run is retained.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct RunState {
    pub run_id: String,
    pub action: QualifiedName,
    pub status: RunStatus,
    /// Populated once `status` is `succeeded`.
    pub result: Option<RunActionResult>,
    /// Populated once `status` is `failed`.
    pub error: Option<String>,
    /// Every progress event emitted so far, in order. Each entry's index is
    /// its SSE event id, so a poller sees the same history a `Last-Event-ID`
    /// SSE reconnect would replay.
    pub progress: Vec<RunActionProgress>,
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

/// Shared `/healthz` body for dobjd and the relayer, synchronizer, and
/// archiver services: the `ok` liveness flag plus the build stamp so a client
/// can tell which build is serving. `version`/`target` are optional so a
/// client can still parse a server built before they were added.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct HealthResponse {
    pub ok: bool,
    /// Release tag this daemon was built from ("dev" outside a release).
    #[serde(default)]
    pub version: Option<String>,
    /// Target triple this daemon was built for (e.g. `aarch64-apple-darwin`),
    /// so a client can confirm which platform's build is serving.
    #[serde(default)]
    pub target: Option<String>,
}

impl HealthResponse {
    /// Liveness OK, stamped with the build the server shipped in (release tag
    /// and target triple from its `build.rs`).
    pub fn stamped(version: &str, target: &str) -> Self {
        Self {
            ok: true,
            version: Some(version.to_string()),
            target: Some(target.to_string()),
        }
    }
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
}
