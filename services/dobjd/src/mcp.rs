//! MCP (Model Context Protocol) operations backed by the same `Arc<Driver>`
//! and broadcast hub used by the HTTP routes.
//!
//! Most methods are thin passes through the driver — the MCP types are
//! aliases for the same `wire-types` definitions the HTTP routes use, so
//! there's nothing to convert.

use std::sync::Arc;

use dobj_mcp::ops::DobjOps;
use dobj_mcp::types as mcp;

use crate::events::EventTx;
use crate::runs::RunRegistry;

pub struct DobjdOps {
    driver: Arc<::driver::Driver>,
    events: EventTx,
    runs: RunRegistry,
}

impl DobjdOps {
    pub fn new(driver: Arc<::driver::Driver>, events: EventTx, runs: RunRegistry) -> Self {
        Self {
            driver,
            events,
            runs,
        }
    }
}

impl DobjOps for DobjdOps {
    fn list_objects(&self) -> anyhow::Result<Vec<mcp::ObjectSummary>> {
        // The driver folds each object's class metadata (emoji, description)
        // into the summary, so clients need no `/classes` round-trip.
        self.driver.sync_objects(None)
    }

    fn list_actions(&self) -> anyhow::Result<Vec<mcp::ActionSummary>> {
        self.driver.list_actions(None)
    }

    fn list_classes(&self) -> anyhow::Result<Vec<mcp::ClassSummary>> {
        self.driver.list_classes()
    }

    fn get_state_root(&self) -> anyhow::Result<String> {
        Ok(payload::encode_hash_hex(&self.driver.get_state_root()?))
    }

    fn inspect_object(&self, file_name: &str) -> anyhow::Result<mcp::ObjectSummary> {
        self.driver.read_object(std::path::Path::new(file_name))
    }

    fn inspect_class(&self, class: &mcp::QualifiedName) -> anyhow::Result<mcp::ClassSummary> {
        self.driver.get_class(class)
    }

    fn inspect_action(&self, action: &mcp::QualifiedName) -> anyhow::Result<mcp::ActionSummary> {
        self.driver.get_action(action)
    }

    fn run_action(&self, input: mcp::RunActionInput) -> anyhow::Result<mcp::RunAccepted> {
        // Pass strings through verbatim — the driver extracts basenames via
        // `Path::file_name`, so an absolute path or a bare basename resolve to
        // the same managed file.
        let input_objects: Vec<String> = input
            .input_object_paths
            .iter()
            .map(|path| path.trim().to_string())
            .collect();

        // The run shares the same registry as the HTTP routes, so the GUI and
        // an agent can both follow the daemon-assigned run id.
        Ok(crate::runs::spawn_run(
            &self.runs,
            self.driver.clone(),
            self.events.clone(),
            input.action,
            input_objects,
        ))
    }

    fn get_run(&self, run_id: &str) -> anyhow::Result<mcp::RunState> {
        self.runs
            .get(run_id)
            .map(|entry| entry.snapshot())
            .ok_or_else(|| anyhow::anyhow!("unknown run: {run_id}"))
    }

    fn check_feasibility(
        &self,
        action: &mcp::QualifiedName,
    ) -> anyhow::Result<mcp::FeasibilityReport> {
        self.driver.check_action(action)
    }

    fn import_object_file(&self, path: &str) -> anyhow::Result<mcp::ObjectSummary> {
        // MCP runs in-process with the driver on the user's machine, so it
        // reads the file here and hands the contents to the same validation
        // core the HTTP route uses. Arbitrary path is fine: the agent already
        // has filesystem access, and import only ingests (never discloses).
        let contents = std::fs::read_to_string(path)
            .map_err(|err| anyhow::anyhow!("could not read .dobj file at {path}: {err}"))?;
        self.driver.import_object(&contents)
    }

    fn read_settings(&self) -> anyhow::Result<mcp::DriverSettings> {
        self.driver.load_settings()
    }

    fn write_settings(&self, settings: mcp::DriverSettings) -> anyhow::Result<mcp::DriverSettings> {
        self.driver.save_settings(&settings)
    }

    fn get_objects_dir(&self) -> anyhow::Result<String> {
        Ok(self
            .driver
            .paths()
            .objects_dir
            .to_string_lossy()
            .to_string())
    }

    fn generated_podlang(&self) -> Option<String> {
        self.driver.generated_podlang()
    }
}
