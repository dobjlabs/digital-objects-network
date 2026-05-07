use std::path::Path;
use std::sync::Arc;

use ::driver::{
    ClassRef, Driver, ExecuteActionInput, ObjectSelector, ObjectStatus, ObjectSummary,
    QualifiedName,
};
use craft_mcp::ops::CraftOps;
use craft_mcp::types as mcp;
use tauri::Emitter;

use crate::progress::TauriProgressReporter;

pub(crate) struct AppCraftOps {
    app: tauri::AppHandle,
    driver: Arc<Driver>,
}

impl AppCraftOps {
    pub(crate) fn new(app: tauri::AppHandle, driver: Arc<Driver>) -> Self {
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
                action: to_mcp_qname(action.action),
                description: action.description,
                total_inputs: action.total_inputs.into_iter().map(to_mcp_class_ref).collect(),
                total_outputs: action.total_outputs.into_iter().map(to_mcp_class_ref).collect(),
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
                produced_by: class_info.produced_by.into_iter().map(to_mcp_qname).collect(),
                consumed_by: class_info.consumed_by.into_iter().map(to_mcp_qname).collect(),
            })
            .collect())
    }

    fn get_state_root(&self) -> anyhow::Result<String> {
        self.driver.get_state_root()
    }

    fn inspect_object(&self, object_id: &str) -> anyhow::Result<mcp::ObjectDetail> {
        let object = self
            .driver
            .read_object(&ObjectSelector::ObjectId(object_id.to_string()))?;
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
            produced_by: class_info.produced_by.into_iter().map(to_mcp_qname).collect(),
            consumed_by: class_info.consumed_by.into_iter().map(to_mcp_qname).collect(),
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
                Ok(ObjectSelector::FileName(selector))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;

        let action_qname = from_mcp_qname(input.action.clone());
        let _ = self.app.emit(
            "mcp-action-started",
            serde_json::json!({ "action": &input.action }),
        );

        let reporter = TauriProgressReporter::new(self.app.clone(), action_qname.id());
        let result = self.driver.execute_with_reporter(
            ExecuteActionInput {
                action: action_qname.clone(),
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
                    .read_object(&ObjectSelector::FileName(file_name.clone()))?;
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
