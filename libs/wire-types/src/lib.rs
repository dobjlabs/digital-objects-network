//! Pure-data wire types shared across the driver, dobjd, MCP, and CLI.
//!
//! Anything that travels over a process boundary (HTTP body, MCP tool
//! parameter, SSE event, CLI arg) belongs here -- but only if it has no
//! logic that depends on ZK proofs or chain state. Keeping this crate
//! dependency-light is the entire point: `cli` and `mcp` should be able
//! to use it without compiling pod2/plonky2/rocksdb/sqlx.
//!
//! ## Submodules
//!
//! - [`relayer`] -- DTOs for the relayer's HTTP API. The status/request
//!   types are pure serde; the two proof-bearing response types carry pod2
//!   `Hash` (`tx_final`, `state_root`), so they live behind the
//!   `chain` feature. The relayer server and the driver enable it.
//! - [`synchronizer`] -- DTOs for the synchronizer's HTTP API. Every type
//!   carries pod2 `Hash` values directly, so the whole module lives behind
//!   the `chain` feature. Only the synchronizer server and the driver speak
//!   this API, and both enable the feature; `cli` and `mcp` don't.
//! - [`transactions`] -- Durable transaction records. Hashes are hex strings,
//!   so these types require no chain dependencies.

pub mod relayer;
#[cfg(feature = "chain")]
pub mod synchronizer;
pub mod transactions;

use std::collections::{BTreeMap, HashMap};
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
    /// Size in bytes of the object's file on disk; absent when it could not
    /// be read.
    #[serde(default)]
    pub file_size: Option<u64>,
    pub class: QualifiedName,
    pub class_hash: String,
    /// The object's class display emoji (`📦` when the class is unknown).
    pub emoji: String,
    pub status: ObjectStatus,
    pub tx_hash: Option<String>,
    /// The object's class description, when its class is known.
    pub description: Option<String>,
    /// Application-layer fields (e.g. `durability`, `key`).
    pub fields: HashMap<String, serde_json::Value>,
    /// The run holding this object as an input, while that run has not yet
    /// succeeded or failed. Reported by dobjd from its run registry and
    /// never stored; `status` still gives the object's on-chain state.
    #[serde(default)]
    pub held_by_run_id: Option<String>,
    /// The object's key is still one it was imported with, so whoever gave
    /// it the file can still spend it. Cleared by any action that replaces
    /// the key, such as the class's built-in Rekey action.
    #[serde(default)]
    pub shared_key: bool,
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
    /// The name to file the object under, typically that of the file it came
    /// in. dobjd adds `.dobj` when it is missing and a numbered suffix when
    /// the name is taken, and refuses an unsafe name. Without one, the object
    /// gets a name derived from its commitment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
}

// ===========================================================================
// Catalog
// ===========================================================================

/// One object an action mutates: its position in `total_inputs` and its
/// position in `total_outputs`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct MutatedObjectSlots {
    pub input_index: usize,
    pub output_index: usize,
}

/// Optional caller data by name, in POD2 JSON form.
pub type ActionArgValues = BTreeMap<String, serde_json::Value>;

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
    /// Path of the `.pexe` archive that declares this action.
    #[serde(default)]
    pub source_path: String,
    /// Objects the action consumes and produces again in updated form,
    /// keeping their stable identifier. Every other output is a new object.
    #[serde(default)]
    pub mutated_objects: Vec<MutatedObjectSlots>,
    /// Podlang source for this action's predicate. Empty if the catalog
    /// can't locate it (shouldn't happen for compiled plugins).
    pub predicate_source: String,
    /// Argument names read during Load; scripts may read other names at runtime.
    #[serde(default)]
    pub args: Vec<String>,
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

/// Action parameters shared by run and proof-job requests.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ActionInput {
    pub action: QualifiedName,
    pub input_object_paths: Vec<String>,
    /// Optional caller data by name, in POD2 JSON form.
    #[serde(default)]
    pub args: ActionArgValues,
}

/// `POST /runs` request body: `{ "input": { ... } }`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct StartRunRequest {
    pub input: ActionInput,
}

/// Result of a successful run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct RunActionResult {
    /// Daemon-assigned id for this run (also the key for
    /// `GET /runs/{run_id}` and the scope of its `run-action-progress`
    /// events).
    pub run_id: String,
    /// Grounding state root against which the transaction was proved.
    pub old_root: String,
    pub output_files: Vec<String>,
    /// The objects the transaction spends; nullified once it lands.
    pub input_files: Vec<String>,
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
    /// Shrinking, recording, submission, and waiting for chain inclusion.
    Committing,
    /// Finished successfully; `RunState::result` is populated.
    Succeeded,
    /// Finished with an error; `RunState::error` is populated.
    Failed,
}

/// `GET /runs` query: `?status=active` limits the list to runs that
/// have not yet succeeded or failed.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct RunListQuery {
    #[serde(default)]
    pub status: Option<RunListFilter>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum RunListFilter {
    Active,
}

/// `POST /runs` response. The run was accepted and now executes in the
/// background; follow it via `GET /runs/{runId}` (poll) or
/// `GET /runs/{runId}/events` (SSE, replayable via `Last-Event-ID`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct RunAccepted {
    pub run_id: String,
    pub status: RunStatus,
    /// When dobjd accepted the run, in Unix epoch milliseconds.
    #[serde(default)]
    pub created_at_ms: u64,
}

/// `GET /runs/{runId}` response: the current state of a run. The
/// poll-and-recover counterpart to the SSE stream — a client that lost its
/// connection re-reads the outcome here for as long as the run is retained.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct RunState {
    pub run_id: String,
    pub action: QualifiedName,
    pub status: RunStatus,
    /// The `.dobj` files the run was given as inputs, as submitted.
    #[serde(default)]
    pub input_object_paths: Vec<String>,
    /// When dobjd accepted the run, in Unix epoch milliseconds.
    #[serde(default)]
    pub created_at_ms: u64,
    /// When the first progress event was recorded, i.e. when the run left
    /// `queued`. Absent while queued.
    #[serde(default)]
    pub started_at_ms: Option<u64>,
    /// When the run reached `succeeded` or `failed`. Absent until then.
    #[serde(default)]
    pub finished_at_ms: Option<u64>,
    /// Monotonic time since acceptance, frozen at completion. No ETA is implied.
    #[serde(default)]
    pub elapsed_ms: u64,
    /// Ordered, non-overlapping stage intervals. A resumed stage has a new entry.
    #[serde(default)]
    pub stage_timings: Vec<RunStageTiming>,
    /// Populated once `status` is `succeeded`.
    pub result: Option<RunActionResult>,
    /// Populated once `status` is `failed`.
    pub error: Option<String>,
    /// Every progress event emitted so far, in order. Each entry's index is
    /// its SSE event id, so a poller sees the same history a `Last-Event-ID`
    /// SSE reconnect would replay.
    pub progress: Vec<RunActionProgress>,
}

/// Proof job lifecycle. `Done` and `Failed` are terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum ProofJobStatus {
    Queued,
    Proving,
    Recording,
    /// Transaction recorded; `tx_final` identifies it.
    Done,
    /// Proof generation or recording failed; see `error`.
    Failed,
}

impl ProofJobStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, ProofJobStatus::Done | ProofJobStatus::Failed)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ProofJobStatus::Queued => "queued",
            ProofJobStatus::Proving => "proving",
            ProofJobStatus::Recording => "recording",
            ProofJobStatus::Done => "done",
            ProofJobStatus::Failed => "failed",
        }
    }
}

impl fmt::Display for ProofJobStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

/// `POST /proof-jobs` response and `GET /proof-jobs/{jobId}` body.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ProofJobState {
    pub job_id: String,
    pub action: QualifiedName,
    pub input_object_paths: Vec<String>,
    pub status: ProofJobStatus,
    /// The latest progress message.
    #[serde(default)]
    pub message: Option<String>,
    /// Set once the job is `done`.
    #[serde(default)]
    pub tx_final: Option<String>,
    /// Set once the job is `failed`.
    #[serde(default)]
    pub error: Option<String>,
    pub created_at_ms: u64,
    #[serde(default)]
    pub started_at_ms: Option<u64>,
    #[serde(default)]
    pub finished_at_ms: Option<u64>,
}

/// Recorded transactions to submit to the relayer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct SubmitTransactionsRequest {
    pub tx_finals: Vec<String>,
}

/// Optional wait for proof completion or transaction landing/expiry.
/// At timeout, return the current state.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct WaitQuery {
    #[serde(default)]
    pub wait: bool,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum ExecutionStage {
    VerifyingInputs,
    LoadingModule,
    Building,
    Mining,
    Solving,
    Proving,
    Shrinking,
    WritingFiles,
    Submitting,
    AwaitingSynchronization,
}

impl ExecutionStage {
    pub fn message(self) -> &'static str {
        match self {
            Self::VerifyingInputs => "Verifying inputs and fetching grounding witness",
            Self::LoadingModule => "Loading action module",
            Self::Building => "Building action and proof statements",
            Self::Mining => "Searching for a proof-of-work key",
            Self::Solving => "Solving proof layout",
            Self::Proving => "Generating ZK proof",
            Self::Shrinking => "Shrinking proof",
            Self::WritingFiles => "Writing output files",
            Self::Submitting => "Submitting proof to relayer",
            Self::AwaitingSynchronization => "Waiting for synchronizer",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct RunStageTiming {
    pub stage: ExecutionStage,
    pub status: ProofProgressStatus,
    pub started_at_ms: u64,
    /// Monotonic duration of this interval; advances while status is running.
    pub elapsed_ms: u64,
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
    pub output_files: Option<Vec<String>>,
    pub output_status: Option<ObjectStatus>,
    pub input_files: Option<Vec<String>>,
    /// When the event was recorded, in Unix epoch milliseconds.
    #[serde(default)]
    pub at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<ExecutionStage>,
    /// Monotonic time since the run was accepted, at this event.
    #[serde(default)]
    pub elapsed_ms: u64,
}

// ===========================================================================
// Driver configuration
// ===========================================================================

/// How hard proving may work the machine. The mechanism, and how much each
/// level saves, differ by platform.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum CpuUsage {
    /// Gives way to other applications further than Medium; where the
    /// platform allows, near-silent and several times slower.
    Light,
    /// Gives way to other applications and, where the platform allows, runs
    /// cooler, at a moderate cost in proving speed.
    #[default]
    Medium,
    /// Proves as fast as the machine allows; other applications may slow
    /// down and fans may spin up.
    Full,
}

impl CpuUsage {
    pub const ALL: [CpuUsage; 3] = [CpuUsage::Light, CpuUsage::Medium, CpuUsage::Full];

    pub fn as_str(&self) -> &'static str {
        match self {
            CpuUsage::Light => "light",
            CpuUsage::Medium => "medium",
            CpuUsage::Full => "full",
        }
    }
}

impl fmt::Display for CpuUsage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for CpuUsage {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        CpuUsage::ALL
            .into_iter()
            .find(|level| level.as_str().eq_ignore_ascii_case(value))
            .ok_or_else(|| format!("unknown CPU usage {value:?}; expected light, medium or full"))
    }
}

/// The persisted configuration shared by the driver and dobjd:
/// synchronizer + relayer URLs, the daemon's MCP server toggle, and how much
/// CPU proving may use.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct DriverSettings {
    pub synchronizer_api_url: String,
    pub relayer_api_url: String,
    /// Whether dobjd serves MCP on the adjacent port (default off).
    #[serde(default)]
    pub mcp_enabled: bool,
    /// How much CPU proving may use (default medium).
    #[serde(default)]
    pub cpu_usage: CpuUsage,
}

/// Partial update for [`DriverSettings`], accepted by `PUT /settings`. Absent
/// fields keep their current persisted value, so a caller can flip one
/// setting without echoing the others. This is why the field type is
/// `Option` rather than reusing `DriverSettings`: there, an omitted
/// `mcpEnabled` would deserialize to `false` (`#[serde(default)]`, needed for
/// reading older settings files) and silently stop the MCP server.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct DriverSettingsPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub synchronizer_api_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relayer_api_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_usage: Option<CpuUsage>,
}

impl DriverSettingsPatch {
    /// Overlay the present fields onto `base`, leaving absent fields untouched.
    pub fn apply_to(self, base: &mut DriverSettings) {
        if let Some(url) = self.synchronizer_api_url {
            base.synchronizer_api_url = url;
        }
        if let Some(url) = self.relayer_api_url {
            base.relayer_api_url = url;
        }
        if let Some(enabled) = self.mcp_enabled {
            base.mcp_enabled = enabled;
        }
        if let Some(cpu_usage) = self.cpu_usage {
            base.cpu_usage = cpu_usage;
        }
    }
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

    #[test]
    fn cpu_usage_parses_any_case() {
        assert_eq!("LIGHT".parse::<CpuUsage>(), Ok(CpuUsage::Light));
        assert_eq!("Full".parse::<CpuUsage>(), Ok(CpuUsage::Full));
        assert!("fast".parse::<CpuUsage>().is_err());
    }

    #[test]
    fn settings_without_cpu_usage_default_to_medium() {
        let settings: DriverSettings =
            serde_json::from_str(r#"{"synchronizerApiUrl":"s","relayerApiUrl":"r"}"#).unwrap();
        assert_eq!(settings.cpu_usage, CpuUsage::Medium);
    }

    #[test]
    fn settings_patch_overlays_cpu_usage_only_when_present() {
        let mut settings: DriverSettings = serde_json::from_str(
            r#"{"synchronizerApiUrl":"s","relayerApiUrl":"r","cpuUsage":"full"}"#,
        )
        .unwrap();
        let patch: DriverSettingsPatch = serde_json::from_str(r#"{"mcpEnabled":true}"#).unwrap();
        patch.apply_to(&mut settings);
        assert_eq!(settings.cpu_usage, CpuUsage::Full);
        let patch: DriverSettingsPatch = serde_json::from_str(r#"{"cpuUsage":"light"}"#).unwrap();
        patch.apply_to(&mut settings);
        assert_eq!(settings.cpu_usage, CpuUsage::Light);
        assert!(settings.mcp_enabled);
    }
}
