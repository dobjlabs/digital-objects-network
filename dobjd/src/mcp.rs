//! MCP (Model Context Protocol) operations backed by the same `Arc<Driver>`
//! and broadcast hub used by the HTTP routes.
//!
//! Ported from `app-gui/src-tauri/src/mcp.rs` — the only differences are:
//! - uses [`SseProgressReporter`] instead of `TauriProgressReporter` for
//!   action execution progress.

use std::path::Path;
use std::sync::Arc;

use craft_mcp::ops::CraftOps;
use craft_mcp::types as mcp;

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
                id: action.id,
                description: action.description,
                total_input_classes: action.total_input_classes,
                total_output_classes: action.total_output_classes,
            })
            .collect())
    }

    fn list_classes(&self) -> anyhow::Result<Vec<mcp::ClassSummary>> {
        Ok(self
            .driver
            .list_classes()?
            .into_iter()
            .map(|class_info| mcp::ClassSummary {
                name: class_info.name,
                live_count: class_info.live_count,
                produced_by: class_info.produced_by,
                consumed_by: class_info.consumed_by,
            })
            .collect())
    }

    fn get_state_root(&self) -> anyhow::Result<String> {
        self.driver.get_state_root()
    }

    fn inspect_object(&self, object_id: &str) -> anyhow::Result<mcp::ObjectDetail> {
        let object = self
            .driver
            .read_object(&::driver::ObjectSelector::ObjectId(object_id.to_string()))?;
        let predicate_source = self
            .driver
            .get_class(&object.class_name)
            .map(|c| c.predicate_source)
            .unwrap_or_default();
        Ok(mcp::ObjectDetail {
            id: object.id,
            class_name: object.class_name,
            status: status_string(object.status),
            tx_hash: object.tx_hash,
            state: object.fields,
            predicate_source,
        })
    }

    fn inspect_class(&self, class_name: &str) -> anyhow::Result<mcp::ClassDetail> {
        let class_info = self.driver.get_class(class_name)?;
        Ok(mcp::ClassDetail {
            class_name: class_info.name,
            predicate_source: class_info.predicate_source,
            produced_by: class_info.produced_by,
            consumed_by: class_info.consumed_by,
        })
    }

    fn run_action(&self, input: mcp::RunActionInput) -> anyhow::Result<mcp::RunActionResult> {
        let input_objects = input
            .input_object_paths
            .iter()
            .map(|path| {
                let path = path.trim();
                let selector = if Path::new(path).is_absolute() {
                    Path::new(path)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .ok_or_else(|| {
                            ::driver::DriverError::InvalidInput(format!(
                                "invalid input path: {path}"
                            ))
                        })?
                        .to_string()
                } else {
                    path.to_string()
                };
                Ok(::driver::ObjectSelector::FileName(selector))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;

        // Generate a per-call run id. action_id is shared across concurrent
        // runs of the same action and isn't unique enough for SSE filtering.
        let run_id = uuid::Uuid::new_v4().to_string();

        let reporter = SseProgressReporter::new(self.events.clone(), run_id.clone());
        let result = match self.driver.execute_with_reporter(
            ::driver::ExecuteActionInput {
                action_id: input.action_id.clone(),
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
                let detail = self
                    .driver
                    .read_object(&::driver::ObjectSelector::FileName(file_name.clone()))?;
                Ok(mcp::InventoryObject {
                    id: detail.id,
                    class_name: detail.class_name,
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
                input.action_id, result.old_root, result.new_root
            ),
            run_id,
            outputs,
            consumed: result.nullified_files,
        })
    }

    fn check_feasibility(&self, action_id: &str) -> anyhow::Result<mcp::FeasibilityReport> {
        let report = self.driver.check_action(action_id)?;
        Ok(mcp::FeasibilityReport {
            feasible: report.feasible,
            action_id: report.action_id,
            available_inputs: report
                .available_inputs
                .into_iter()
                .map(|candidate| mcp::FeasibilityInput {
                    class_name: candidate.class_name,
                    object_id: candidate.object_id,
                    file_name: candidate.file_name,
                })
                .collect(),
            missing_inputs: report.missing_inputs,
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

fn status_string(status: ::driver::ObjectStatus) -> String {
    match status {
        ::driver::ObjectStatus::Unknown => "unknown",
        ::driver::ObjectStatus::Pending => "pending",
        ::driver::ObjectStatus::Live => "live",
        ::driver::ObjectStatus::Nullified => "nullified",
    }
    .to_string()
}

fn to_mcp_inventory_object(object: ::driver::ObjectSummary) -> mcp::InventoryObject {
    mcp::InventoryObject {
        id: object.id,
        class_name: object.class_name,
        file_name: object.file_name,
        status: status_string(object.status),
        tx_hash: object.tx_hash,
        fields: object.fields,
    }
}
