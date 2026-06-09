use crate::types::*;

/// The interface the MCP server needs from the host process.
/// In production it's implemented by `DobjdOps` in the `dobjd` crate
/// (one driver process serving HTTP + MCP). Tests use `MockDobjOps`.
pub trait DobjOps: Send + Sync + 'static {
    fn list_objects(&self) -> anyhow::Result<Vec<ObjectSummary>>;
    fn list_actions(&self) -> anyhow::Result<Vec<Action>>;
    fn list_classes(&self) -> anyhow::Result<Vec<ClassSummary>>;
    fn get_state_root(&self) -> anyhow::Result<String>;
    fn inspect_object(&self, file_name: &str) -> anyhow::Result<ObjectDetail>;
    fn inspect_class(&self, class: &QualifiedName) -> anyhow::Result<ClassDetail>;
    fn inspect_action(&self, action: &QualifiedName) -> anyhow::Result<ActionDetail>;
    /// Start a run in the background and return its handle immediately. The
    /// proof + commit pipeline runs asynchronously; follow it with `get_run`.
    fn run_action(&self, input: RunActionInput) -> anyhow::Result<RunAccepted>;
    /// Current state of a previously-started run, by its run id.
    fn get_run(&self, run_id: &str) -> anyhow::Result<RunState>;
    fn check_feasibility(&self, action: &QualifiedName) -> anyhow::Result<FeasibilityReport>;

    /// Import an external `.dobj` object — one not produced by this driver —
    /// into the local object store by reading it from a local filesystem path.
    /// Returns the filed object's summary.
    fn import_object_file(&self, path: &str) -> anyhow::Result<ObjectDetail>;

    /// Read the driver's current configuration (synchronizer + relayer URLs).
    fn read_settings(&self) -> anyhow::Result<DriverSettings>;
    /// Persist updated driver configuration. Implementations should accept
    /// only the URLs we expose — the schema is intentionally minimal.
    fn write_settings(&self, settings: DriverSettings) -> anyhow::Result<DriverSettings>;
    /// Filesystem path to the objects directory (`~/.dobj/objects/`).
    fn get_objects_dir(&self) -> anyhow::Result<String>;

    /// Returns the generated podlang source for all actions and classes,
    /// or None if not available (e.g. in mock mode).
    fn generated_podlang(&self) -> Option<String> {
        None
    }
}
