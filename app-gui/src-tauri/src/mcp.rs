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
                display_name: action.display_name,
                plugin_name: action.plugin_name,
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
                id: class_info.id,
                display_name: class_info.display_name,
                plugin_name: class_info.plugin_name,
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
            .get_class(&object.class_id)
            .map(|c| c.predicate_source)
            .unwrap_or_default();
        Ok(mcp::ObjectDetail {
            id: object.id,
            class_id: object.class_id,
            class_display_name: object.class_display_name,
            plugin_name: object.plugin_name,
            status: status_string(object.status),
            tx_hash: object.tx_hash,
            state: object.fields,
            predicate_source,
        })
    }

    fn inspect_class(&self, class_id: &str) -> anyhow::Result<mcp::ClassDetail> {
        let class_info = self.driver.get_class(class_id)?;
        Ok(mcp::ClassDetail {
            class_id: class_info.id,
            class_display_name: class_info.display_name,
            plugin_name: class_info.plugin_name,
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

        let _ = self.app.emit(
            "mcp-action-started",
            serde_json::json!({ "actionId": input.action_id }),
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
                    class_id: detail.class_id,
                    class_display_name: detail.class_display_name,
                    plugin_name: detail.plugin_name,
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
                    class_id: candidate.class_id,
                    class_display_name: candidate.class_display_name,
                    plugin_name: candidate.plugin_name,
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
        class_id: object.class_id,
        class_display_name: object.class_display_name,
        plugin_name: object.plugin_name,
        file_name: object.file_name,
        status: status_string(object.status),
        tx_hash: object.tx_hash,
        fields: object.fields,
    }
}

fn to_mcp_class_ref(r: ::driver::ClassRef) -> mcp::ClassRef {
    mcp::ClassRef {
        id: r.id,
        display_name: r.display_name,
        hash: r.hash,
    }
}
