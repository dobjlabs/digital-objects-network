use super::{
    object_store::{ensure_objects_dirs, load_object_files},
};
use crate::objects::objects_dir;
use serde::Serialize;

use crate::{objects::ObjectRecord, spec};

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct InventoryObject {
    pub id: String,
    pub file_name: String,
    pub class_name: String,
    pub emoji: String,
    pub nullifier: Option<String>,
    pub description: Option<String>,
    pub obj: serde_json::Value,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Action {
    pub id: String,
    pub emoji: String,
    pub description: String,
    pub cpu_cost: String,
    pub reads_block: bool,
    pub input_classes: Vec<String>,
}

pub(super) fn build_action_catalog() -> Vec<Action> {
    spec::visible_action_descriptors()
        .into_iter()
        .map(|descriptor| Action {
            id: descriptor.name,
            emoji: descriptor.ui.emoji.to_string(),
            description: descriptor.ui.description.to_string(),
            cpu_cost: descriptor.ui.cpu_cost.to_string(),
            reads_block: descriptor.ui.reads_block,
            input_classes: descriptor.input_classes,
        })
        .collect()
}

pub(super) fn to_inventory_object(record: &ObjectRecord, file_name: &str) -> InventoryObject {
    let class_ui = spec::class_ui_meta(&record.class_name);
    InventoryObject {
        id: record.id.clone(),
        file_name: file_name.to_string(),
        class_name: record.class_name.clone(),
        emoji: class_ui.emoji.to_string(),
        nullifier: record.nullifier.clone(),
        description: Some(class_ui.description.to_string()),
        obj: serde_json::to_value(&record.obj).expect("object dictionary should serialize"),
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadGuiInventoryResult {
    pub inventory: Vec<InventoryObject>,
    pub actions: Vec<Action>,
}

#[tauri::command]
pub async fn load_gui_inventory(app: tauri::AppHandle) -> Result<LoadGuiInventoryResult, String> {
    let objects_dir = objects_dir(&app)?;
    ensure_objects_dirs(&objects_dir)?;
    let objects = load_object_files(&objects_dir)?;
    let actions = build_action_catalog();

    Ok(LoadGuiInventoryResult {
        inventory: objects
            .iter()
            .map(|entry| to_inventory_object(&entry.record, &entry.file_name))
            .collect(),
        actions,
    })
}
