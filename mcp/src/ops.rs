use crate::types::*;

/// The interface the MCP server needs from the host process.
/// In production it's implemented by `DobjdCraftOps` in the `dobjd` crate
/// (one driver process serving HTTP + MCP). Tests use `MockCraftOps`.
pub trait CraftOps: Send + Sync + 'static {
    fn list_inventory(&self) -> anyhow::Result<Vec<InventoryObject>>;
    fn list_actions(&self) -> anyhow::Result<Vec<Action>>;
    fn list_classes(&self) -> anyhow::Result<Vec<ClassSummary>>;
    fn get_state_root(&self) -> anyhow::Result<String>;
    fn inspect_object(&self, object_id: &str) -> anyhow::Result<ObjectDetail>;
    fn inspect_class(&self, class_name: &str) -> anyhow::Result<ClassDetail>;
    fn run_action(&self, input: RunActionInput) -> anyhow::Result<RunActionResult>;
    fn check_feasibility(&self, action_id: &str) -> anyhow::Result<FeasibilityReport>;

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
