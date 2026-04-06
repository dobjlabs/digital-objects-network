use std::path::Path;
use std::sync::Arc;

use craft_mcp::ops::CraftOps;
use craft_mcp::types as mcp;
use tauri::Emitter;

use crate::progress::TauriProgressReporter;

pub(crate) struct AppCraftOps {
    app: tauri::AppHandle,
    driver: Arc<::driver::Driver>,
}

impl AppCraftOps {
    pub(crate) fn new(app: tauri::AppHandle, driver: Arc<::driver::Driver>) -> Self {
        Self { app, driver }
    }
}

impl CraftOps for AppCraftOps {
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
                input_classes: action.input_classes,
                output_classes: action.output_classes,
                cpu_cost: action.cpu_cost,
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
        Ok(mcp::ObjectDetail {
            id: object.id,
            class_name: object.class_name,
            live: object.live,
            state: object.fields,
            predicate_source: object.predicate_source,
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
                        .ok_or_else(|| anyhow::anyhow!("invalid input path: {path}"))?
                        .to_string()
                } else {
                    path.to_string()
                };
                Ok(::driver::ObjectSelector::FileName(selector))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;

        let cpu_cost = self
            .driver
            .list_actions(None)?
            .into_iter()
            .find(|action| action.id == input.action_id)
            .map(|action| action.cpu_cost)
            .unwrap_or_default();
        let _ = self.app.emit(
            "mcp-action-started",
            serde_json::json!({
                "actionId": input.action_id,
                "cpuCost": cpu_cost,
            }),
        );

        let reporter = TauriProgressReporter::new(self.app.clone(), input.action_id.clone());
        let result = self.driver.execute_with_reporter(
            ::driver::ExecuteActionInput {
                action_id: input.action_id.clone(),
                input_objects,
            },
            &reporter,
        )?;

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
                    live: detail.live,
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

    fn generated_podlang(&self) -> Option<String> {
        self.driver.generated_podlang()
    }
}

fn to_mcp_inventory_object(object: ::driver::ObjectSummary) -> mcp::InventoryObject {
    mcp::InventoryObject {
        id: object.id,
        class_name: object.class_name,
        file_name: object.file_name,
        live: object.live,
        fields: object.fields,
    }
}
