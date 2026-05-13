//! MCP (Model Context Protocol) operations backed by the same `Arc<Driver>`
//! and broadcast hub used by the HTTP routes.
//!
//! Ported from `app-gui/src-tauri/src/mcp.rs` — the only differences are:
//! - uses [`SseProgressReporter`] instead of `TauriProgressReporter` for
//!   action execution progress.

use std::sync::Arc;

use ::driver::{ExecuteActionInput, ObjectStatus, ObjectSummary};
use craft_mcp::ops::CraftOps;
use craft_mcp::types as mcp;
use wire_types::{ClassRef, QualifiedName};

use crate::events::EventTx;
use crate::progress::SseProgressReporter;

pub struct DobjdCraftOps {
    driver: Arc<::driver::Driver>,
    events: EventTx,
}

impl DobjdCraftOps {
    pub fn new(driver: Arc<::driver::Driver>, events: EventTx) -> Self {
        Self { driver, events }
    }
}

impl CraftOps for DobjdCraftOps {
    fn list_inventory(&self) -> anyhow::Result<Vec<mcp::InventoryObject>> {
        Ok(self
            .driver
            .sync_inventory(None)?
            .into_iter()
            .map(to_mcp_inventory_object)
            .collect())
    }

    fn list_actions(&self) -> anyhow::Result<Vec<mcp::Action>> {
        Ok(self
            .driver
            .list_actions(None)?
            .into_iter()
            .map(|action| mcp::Action {
                action: to_mcp_qname(action.action),
                description: action.description,
                total_inputs: action
                    .total_inputs
                    .into_iter()
                    .map(to_mcp_class_ref)
                    .collect(),
                total_outputs: action
                    .total_outputs
                    .into_iter()
                    .map(to_mcp_class_ref)
                    .collect(),
            })
            .collect())
    }

    fn list_classes(&self) -> anyhow::Result<Vec<mcp::ClassSummary>> {
        Ok(self
            .driver
            .list_classes()?
            .into_iter()
            .map(|class_info| mcp::ClassSummary {
                class: to_mcp_qname(class_info.class),
                live_count: class_info.live_count,
                produced_by: class_info
                    .produced_by
                    .into_iter()
                    .map(to_mcp_qname)
                    .collect(),
                consumed_by: class_info
                    .consumed_by
                    .into_iter()
                    .map(to_mcp_qname)
                    .collect(),
            })
            .collect())
    }

    fn get_state_root(&self) -> anyhow::Result<String> {
        self.driver.get_state_root()
    }

    fn inspect_object(&self, file_name: &str) -> anyhow::Result<mcp::ObjectDetail> {
        let object = self.driver.read_object(std::path::Path::new(file_name))?;
        let predicate_source = self
            .driver
            .get_class(&object.class)
            .map(|c| c.predicate_source)
            .unwrap_or_default();
        Ok(mcp::ObjectDetail {
            id: object.id,
            class: to_mcp_qname(object.class),
            status: status_string(object.status),
            tx_hash: object.tx_hash,
            state: object.fields,
            predicate_source,
        })
    }

    fn inspect_class(&self, class: &mcp::QualifiedName) -> anyhow::Result<mcp::ClassDetail> {
        let class_info = self.driver.get_class(&from_mcp_qname(class.clone()))?;
        Ok(mcp::ClassDetail {
            class: to_mcp_qname(class_info.class),
            predicate_source: class_info.predicate_source,
            produced_by: class_info
                .produced_by
                .into_iter()
                .map(to_mcp_qname)
                .collect(),
            consumed_by: class_info
                .consumed_by
                .into_iter()
                .map(to_mcp_qname)
                .collect(),
        })
    }

    fn inspect_action(&self, action: &mcp::QualifiedName) -> anyhow::Result<mcp::ActionDetail> {
        let summary = self.driver.get_action(&from_mcp_qname(action.clone()))?;
        Ok(mcp::ActionDetail {
            id: summary.action.name,
            description: summary.description,
            total_input_classes: summary
                .total_inputs
                .into_iter()
                .map(|r| r.class.name)
                .collect(),
            total_output_classes: summary
                .total_outputs
                .into_iter()
                .map(|r| r.class.name)
                .collect(),
            predicate_source: summary.predicate_source,
        })
    }

    fn run_action(&self, input: mcp::RunActionInput) -> anyhow::Result<mcp::RunActionResult> {
        // Pass strings through verbatim — the driver extracts basenames
        // via `Path::file_name`, so an absolute path or a bare basename
        // resolve to the same managed file.
        let input_objects: Vec<String> = input
            .input_object_paths
            .iter()
            .map(|path| path.trim().to_string())
            .collect();

        let action_qname = from_mcp_qname(input.action.clone());

        // Generate a per-call run id. The action qualified name is shared
        // across concurrent runs of the same action and isn't unique enough
        // for SSE filtering on the client.
        let run_id = uuid::Uuid::new_v4().to_string();

        let reporter = SseProgressReporter::new(self.events.clone(), run_id.clone());
        let result = match self.driver.execute_with_reporter(
            ExecuteActionInput {
                action: action_qname.clone(),
                input_objects,
            },
            &reporter,
        ) {
            Ok(result) => result,
            Err(err) => {
                reporter.commit_failed(err.to_string());
                return Err(err);
            }
        };

        let outputs = result
            .output_files
            .iter()
            .map(|file_name| {
                let detail = self.driver.read_object(std::path::Path::new(file_name))?;
                Ok(mcp::InventoryObject {
                    id: detail.id,
                    class: to_mcp_qname(detail.class),
                    file_name: detail.file_name,
                    status: status_string(detail.status),
                    tx_hash: detail.tx_hash,
                    fields: detail.fields,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;

        Ok(mcp::RunActionResult {
            success: true,
            message: format!(
                "Action {} completed. Old root: {}, New root: {}",
                action_qname, result.old_root, result.new_root
            ),
            run_id,
            outputs,
            consumed: result.nullified_files,
        })
    }

    fn check_feasibility(
        &self,
        action: &mcp::QualifiedName,
    ) -> anyhow::Result<mcp::FeasibilityReport> {
        let report = self.driver.check_action(&from_mcp_qname(action.clone()))?;
        Ok(mcp::FeasibilityReport {
            feasible: report.feasible,
            action: to_mcp_qname(report.action),
            available_inputs: report
                .available_inputs
                .into_iter()
                .map(|candidate| mcp::FeasibilityInput {
                    class: to_mcp_qname(candidate.class),
                    object_id: candidate.object_id,
                    file_name: candidate.file_name,
                })
                .collect(),
            missing_inputs: report
                .missing_inputs
                .into_iter()
                .map(to_mcp_class_ref)
                .collect(),
        })
    }

    fn read_settings(&self) -> anyhow::Result<mcp::DriverSettings> {
        let s = self.driver.load_settings()?;
        Ok(mcp::DriverSettings {
            synchronizer_api_url: s.synchronizer_api_url,
            relayer_api_url: s.relayer_api_url,
        })
    }

    fn write_settings(&self, settings: mcp::DriverSettings) -> anyhow::Result<mcp::DriverSettings> {
        let saved = self.driver.save_settings(&::driver::DriverSettings {
            synchronizer_api_url: settings.synchronizer_api_url,
            relayer_api_url: settings.relayer_api_url,
        })?;
        Ok(mcp::DriverSettings {
            synchronizer_api_url: saved.synchronizer_api_url,
            relayer_api_url: saved.relayer_api_url,
        })
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

fn status_string(status: ObjectStatus) -> String {
    match status {
        ObjectStatus::Unknown => "unknown",
        ObjectStatus::Pending => "pending",
        ObjectStatus::Live => "live",
        ObjectStatus::Nullified => "nullified",
    }
    .to_string()
}

fn to_mcp_inventory_object(object: ObjectSummary) -> mcp::InventoryObject {
    mcp::InventoryObject {
        id: object.id,
        class: to_mcp_qname(object.class),
        file_name: object.file_name,
        status: status_string(object.status),
        tx_hash: object.tx_hash,
        fields: object.fields,
    }
}

fn to_mcp_class_ref(r: ClassRef) -> mcp::ClassRef {
    mcp::ClassRef {
        class: to_mcp_qname(r.class),
        hash: r.hash,
    }
}

fn to_mcp_qname(q: QualifiedName) -> mcp::QualifiedName {
    mcp::QualifiedName {
        plugin_name: q.plugin_name,
        name: q.name,
    }
}

fn from_mcp_qname(q: mcp::QualifiedName) -> QualifiedName {
    QualifiedName::new(q.plugin_name, q.name)
}
