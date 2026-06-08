//! MCP (Model Context Protocol) operations backed by the same `Arc<Driver>`
//! and broadcast hub used by the HTTP routes.
//!
//! Most methods are thin passes through the driver — the MCP types are
//! aliases for the same `wire-types` definitions the HTTP routes use, so
//! there's nothing to convert.

use std::collections::HashMap;
use std::sync::Arc;

use craft_mcp::ops::CraftOps;
use craft_mcp::types as mcp;

use crate::events::EventTx;
use crate::runs::RunRegistry;

pub struct DobjdCraftOps {
    driver: Arc<::driver::Driver>,
    events: EventTx,
    runs: RunRegistry,
}

impl DobjdCraftOps {
    pub fn new(driver: Arc<::driver::Driver>, events: EventTx, runs: RunRegistry) -> Self {
        Self {
            driver,
            events,
            runs,
        }
    }
}

impl CraftOps for DobjdCraftOps {
    fn list_inventory(&self) -> anyhow::Result<Vec<mcp::InventoryObject>> {
        // Driver returns the basic `ObjectSummary`; the inventory wire
        // shape folds in per-class metadata (emoji, description) so
        // clients can render rows without a `/classes` round-trip. Same
        // logic as `routes::inventory::load_inventory`.
        let classes = self
            .driver
            .list_classes()?
            .into_iter()
            .map(|c| (c.class.clone(), c))
            .collect::<HashMap<_, _>>();

        Ok(self
            .driver
            .sync_inventory(None)?
            .into_iter()
            .map(|object| {
                let class_info = classes.get(&object.class);
                mcp::InventoryObject {
                    content_hash: object.content_hash,
                    file_name: object.file_name,
                    class: object.class.clone(),
                    class_hash: object.class_hash,
                    emoji: class_info
                        .map(|c| c.emoji.clone())
                        .unwrap_or_else(|| "📦".to_string()),
                    status: object.status,
                    tx_hash: object.tx_hash,
                    description: class_info.map(|c| c.description.clone()),
                    fields: object.fields,
                }
            })
            .collect())
    }

    fn list_actions(&self) -> anyhow::Result<Vec<mcp::Action>> {
        self.driver.list_actions(None)
    }

    fn list_classes(&self) -> anyhow::Result<Vec<mcp::ClassSummary>> {
        self.driver.list_classes()
    }

    fn get_state_root(&self) -> anyhow::Result<String> {
        Ok(common::encode_hash_hex(&self.driver.get_state_root()?))
    }

    fn inspect_object(&self, file_name: &str) -> anyhow::Result<mcp::ObjectDetail> {
        self.driver.read_object(std::path::Path::new(file_name))
    }

    fn inspect_class(&self, class: &mcp::QualifiedName) -> anyhow::Result<mcp::ClassDetail> {
        self.driver.get_class(class)
    }

    fn inspect_action(&self, action: &mcp::QualifiedName) -> anyhow::Result<mcp::ActionDetail> {
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

        // Use the client-supplied run id if present, otherwise mint one. Same
        // convention as the HTTP `/actions/run` route, and the run shares the
        // same registry — an agent and the GUI can follow the same run id.
        let run_id = input
            .run_id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        Ok(crate::runs::spawn_run(
            &self.runs,
            self.driver.clone(),
            self.events.clone(),
            run_id,
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

    fn import_object_file(&self, path: &str) -> anyhow::Result<mcp::ObjectDetail> {
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
