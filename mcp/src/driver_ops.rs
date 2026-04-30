//! Production [`CraftOps`] backed by a [`driver::Driver`].
//!
//! Wraps the new risc0-stack driver to satisfy the MCP server's tool
//! interface. The agent calling MCP tools ends up driving the same code
//! path as the Tauri GUI's `run_action` command.
//!
//! Concurrency: `run_action` is wrapped in a `Mutex<()>` to enforce
//! single-action-at-a-time semantics — same UX as the rhai-era driver
//! used to provide via its `Executor`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::{Result, anyhow};
use driver::{Driver, ObjectStatus, all_actions, all_classes};
use driver::driver::ObjectSelector;
use std::path::Path;

use crate::ops::CraftOps;
use crate::types::*;

pub struct DriverCraftOps {
    driver: Arc<Driver>,
    action_lock: Mutex<()>,
}

impl DriverCraftOps {
    pub fn new(driver: Arc<Driver>) -> Self {
        Self {
            driver,
            action_lock: Mutex::new(()),
        }
    }

    fn class_description(&self, class_name: &str) -> String {
        all_classes()
            .iter()
            .find(|c| c.name == class_name)
            .map(|c| c.description.to_string())
            .unwrap_or_else(|| format!("Class {class_name}"))
    }

    fn list_objects_inv(&self) -> Result<Vec<InventoryObject>> {
        let records = self.driver.list_objects()?;
        Ok(records
            .into_iter()
            .map(|r| InventoryObject {
                id: r.id.clone(),
                file_name: ::driver::object::file_name_for(&r.class_name, r.commitment()),
                class_name: r.class_name.clone(),
                status: status_str(r.status),
                tx_hash: r.tx_hash.clone(),
                fields: object_fields_to_json(&r.obj.fields),
            })
            .collect())
    }
}

impl CraftOps for DriverCraftOps {
    fn list_inventory(&self) -> Result<Vec<InventoryObject>> {
        self.list_objects_inv()
    }

    fn list_actions(&self) -> Result<Vec<Action>> {
        Ok(all_actions()
            .iter()
            .filter(|a| !a.hidden)
            .map(|a| Action {
                id: a.name.to_string(),
                description: a.description.to_string(),
                input_classes: a.inputs.iter().map(|s| s.to_string()).collect(),
                output_classes: a.outputs.iter().map(|s| s.to_string()).collect(),
            })
            .collect())
    }

    fn list_classes(&self) -> Result<Vec<ClassSummary>> {
        let live = self.list_objects_inv()?;
        let actions = all_actions();
        let mut out: Vec<ClassSummary> = all_classes()
            .iter()
            .map(|c| {
                let live_count = live
                    .iter()
                    .filter(|o| o.class_name == c.name && o.status == "live")
                    .count();
                let produced_by = actions
                    .iter()
                    .filter(|a| a.outputs.contains(&c.name))
                    .map(|a| a.name.to_string())
                    .collect();
                let consumed_by = actions
                    .iter()
                    .filter(|a| a.inputs.contains(&c.name))
                    .map(|a| a.name.to_string())
                    .collect();
                ClassSummary {
                    name: c.name.to_string(),
                    live_count,
                    produced_by,
                    consumed_by,
                }
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    fn get_state_root(&self) -> Result<String> {
        let head = self.driver.deps.synchronizer.state_head()?;
        Ok(head
            .current_gsr
            .map(|h| format!("{h}"))
            .unwrap_or_else(|| "0x".to_string()))
    }

    fn inspect_object(&self, object_id: &str) -> Result<ObjectDetail> {
        let inv = self.list_objects_inv()?;
        let obj = inv
            .into_iter()
            .find(|o| o.id == object_id)
            .ok_or_else(|| anyhow!("object not found: {object_id}"))?;
        Ok(ObjectDetail {
            description: self.class_description(&obj.class_name),
            id: obj.id,
            class_name: obj.class_name,
            status: obj.status,
            tx_hash: obj.tx_hash,
            state: obj.fields,
        })
    }

    fn inspect_class(&self, class_name: &str) -> Result<ClassDetail> {
        if !all_classes().iter().any(|c| c.name == class_name) {
            return Err(anyhow!("unknown class: {class_name}"));
        }
        let actions = all_actions();
        let produced_by = actions
            .iter()
            .filter(|a| a.outputs.contains(&class_name))
            .map(|a| a.name.to_string())
            .collect();
        let consumed_by = actions
            .iter()
            .filter(|a| a.inputs.contains(&class_name))
            .map(|a| a.name.to_string())
            .collect();
        Ok(ClassDetail {
            class_name: class_name.to_string(),
            description: self.class_description(class_name),
            produced_by,
            consumed_by,
        })
    }

    fn run_action(&self, input: RunActionInput) -> Result<RunActionResult> {
        // Single-flight: refuse if another action is already in progress.
        let _guard = self
            .action_lock
            .try_lock()
            .map_err(|_| anyhow!("an action is already in progress"))?;

        let selectors: Vec<ObjectSelector> = input
            .input_object_paths
            .iter()
            .map(|raw| {
                let trimmed = raw.trim();
                let file_name = if Path::new(trimmed).is_absolute() {
                    Path::new(trimmed)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .ok_or_else(|| anyhow!("invalid input path: {trimmed}"))?
                        .to_string()
                } else {
                    trimmed.to_string()
                };
                Ok::<_, anyhow::Error>(ObjectSelector::FileName(file_name))
            })
            .collect::<Result<Vec<_>>>()?;

        let result = self.driver.execute_named(&input.action_id, selectors)?;

        // Re-read inventory + filter for the freshly-produced outputs.
        let inv_after = self.list_objects_inv()?;
        let outputs: Vec<InventoryObject> = inv_after
            .into_iter()
            .filter(|o| result.output_files.iter().any(|f| f == &o.file_name))
            .collect();

        Ok(RunActionResult {
            success: true,
            message: format!(
                "Action {} produced {} object(s), consumed {}",
                result.action_name,
                outputs.len(),
                result.nullified_files.len()
            ),
            outputs,
            consumed: result.nullified_files,
        })
    }

    fn check_feasibility(&self, action_id: &str) -> Result<FeasibilityReport> {
        let action = all_actions()
            .iter()
            .find(|a| a.name == action_id)
            .ok_or_else(|| anyhow!("unknown action: {action_id}"))?;
        let inv = self.list_objects_inv()?;
        let mut available = Vec::new();
        let mut missing = Vec::new();
        // Match each required input class against an unused live object.
        let mut used_ids = std::collections::HashSet::new();
        for required in action.inputs {
            if let Some(obj) = inv
                .iter()
                .find(|o| &o.class_name == required && o.status == "live" && !used_ids.contains(&o.id))
            {
                used_ids.insert(obj.id.clone());
                available.push(FeasibilityInput {
                    class_name: obj.class_name.clone(),
                    object_id: obj.id.clone(),
                    file_name: obj.file_name.clone(),
                });
            } else {
                missing.push(required.to_string());
            }
        }
        Ok(FeasibilityReport {
            feasible: missing.is_empty(),
            action_id: action_id.to_string(),
            available_inputs: available,
            missing_inputs: missing,
        })
    }
}

fn status_str(s: ObjectStatus) -> String {
    match s {
        ObjectStatus::Unknown => "unknown",
        ObjectStatus::Pending => "pending",
        ObjectStatus::Live => "live",
        ObjectStatus::Nullified => "nullified",
    }
    .to_string()
}

fn object_fields_to_json(
    fields: &std::collections::BTreeMap<String, txlib_core::Value>,
) -> HashMap<String, serde_json::Value> {
    fields
        .iter()
        .map(|(k, v)| {
            let json = serde_json::to_value(v).unwrap_or(serde_json::Value::Null);
            (k.clone(), json)
        })
        .collect()
}
