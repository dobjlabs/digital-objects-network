use super::object_store::{ensure_objects_dirs, load_object_files};
use crate::objects::objects_dir;
use craft_sdk::Helper;
use pod2::middleware::Hash;
use serde::Serialize;
use std::collections::HashMap;

use crate::{objects::ObjectRecord, settings::get_app_settings, spec};

use super::synchronizer_client::{encode_hash_hex, fetch_synchronizer_state};

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct InventoryObject {
    pub id: String,
    pub file_name: String,
    pub class_name: String,
    pub class_hash: String,
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
    pub hash: String,
    pub input_class_hashes: Vec<String>,
    pub description: String,
    pub cpu_cost: String,
    pub reads_block: bool,
    pub input_classes: Vec<String>,
}

pub(super) fn build_action_catalog(
    action_hashes: &HashMap<String, Hash>,
    class_hashes: &HashMap<String, Hash>,
) -> Vec<Action> {
    spec::visible_action_descriptors()
        .into_iter()
        .map(|descriptor| Action {
            id: descriptor.name.clone(),
            emoji: descriptor.ui.emoji.to_string(),
            hash: action_hashes
                .get(&descriptor.name)
                .map(|hash| format!("{:#}", hash))
                .unwrap_or_default(),
            input_class_hashes: descriptor
                .input_classes
                .iter()
                .map(|class_name| {
                    class_hashes
                        .get(class_name)
                        .map(|hash| format!("{:#}", hash))
                        .unwrap_or_default()
                })
                .collect(),
            description: descriptor.ui.description.to_string(),
            cpu_cost: descriptor.ui.cpu_cost.to_string(),
            reads_block: descriptor.ui.reads_block,
            input_classes: descriptor.input_classes,
        })
        .collect()
}

pub(super) fn to_inventory_object(
    record: &ObjectRecord,
    file_name: &str,
    class_hashes: &HashMap<String, Hash>,
) -> InventoryObject {
    let class_ui = spec::class_ui_meta(&record.class_name);
    InventoryObject {
        id: record.id.clone(),
        file_name: file_name.to_string(),
        class_name: record.class_name.clone(),
        class_hash: class_hashes
            .get(&record.class_name)
            .map(|hash| format!("{:#}", hash))
            .unwrap_or_default(),
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
    let helper = Helper::new(spec::dependencies(), spec::actions());
    let action_hashes = helper.action_hashes();
    let class_hashes = helper.class_hashes();
    let actions = build_action_catalog(&action_hashes, &class_hashes);

    Ok(LoadGuiInventoryResult {
        inventory: objects
            .iter()
            .map(|entry| to_inventory_object(&entry.record, &entry.file_name, &class_hashes))
            .collect(),
        actions,
    })
}

#[tauri::command]
pub async fn get_global_state_root(app: tauri::AppHandle) -> Result<String, String> {
    let app_settings = get_app_settings(app)?;
    let sync_state = fetch_synchronizer_state(&app_settings.synchronizer_api_url)?;
    Ok(encode_hash_hex(&sync_state.current_gsr))
}
