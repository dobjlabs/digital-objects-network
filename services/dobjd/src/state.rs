use std::path::Path;
use std::sync::Arc;

use driver::Driver;
use wire_types::ObjectSummary;
use wire_types::transactions::TransactionRecord;

use crate::events::{Event, EventHub};
use crate::jobs::JobRegistry;
use crate::mcp::McpRuntime;
use crate::runs::RunRegistry;
use crate::settle::Settler;

/// What the HTTP routes, the MCP server and the background workers share.
#[derive(Clone)]
pub struct Services {
    pub driver: Arc<Driver>,
    pub events: EventHub,
    pub runs: RunRegistry,
    pub jobs: JobRegistry,
    pub settler: Arc<Settler>,
}

#[derive(Clone)]
pub struct AppState {
    pub services: Services,
    pub mcp: Arc<McpRuntime>,
}

impl AppState {
    pub fn new(services: Services, mcp: Arc<McpRuntime>) -> Self {
        Self { services, mcp }
    }
}

impl Services {
    /// Basename to the id of the run or proof job holding it as an input:
    /// the driver's hold table while a proof is in progress, then the run
    /// registry while a run follows its transaction to the chain.
    pub fn fill_held_by(&self, objects: &mut [ObjectSummary]) {
        crate::runs::fill_held_by(&self.driver, &self.runs, objects);
    }

    pub fn object_summary(&self, file_name: &str) -> anyhow::Result<ObjectSummary> {
        let mut summary = self.driver.read_object(Path::new(file_name))?;
        self.fill_held_by(std::slice::from_mut(&mut summary));
        Ok(summary)
    }

    pub fn publish_objects(&self, file_names: &[String]) {
        for file_name in file_names {
            match self.object_summary(file_name) {
                Ok(summary) => {
                    self.events.publish(Event::Object(summary));
                }
                Err(err) => tracing::debug!("no summary for {file_name}: {err:#}"),
            }
        }
    }

    pub fn announce_transaction(&self, record: &TransactionRecord) {
        self.events.publish(Event::Transaction(record.clone()));
        self.publish_objects(&record.output_files);
        self.publish_objects(&record.input_files);
        self.settler.wake();
    }
}
