use std::collections::HashMap;
use std::sync::Arc;

use crate::error::CommandError;
use serde::Serialize;

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct InventoryObject {
    pub id: String,
    pub file_name: String,
    pub class_id: String,
    pub class_display_name: String,
    pub plugin_name: String,
    pub class_hash: String,
    pub emoji: String,
    pub status: driver::ObjectStatus,
    pub tx_hash: Option<String>,
    pub grounded: bool,
    pub description: Option<String>,
    pub obj: serde_json::Value,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Action {
    pub id: String,
    pub display_name: String,
    pub plugin_name: String,
    pub emoji: String,
    pub hash: String,
    pub total_inputs: Vec<::driver::ClassRef>,
    pub description: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadGuiInventoryResult {
    pub inventory: Vec<InventoryObject>,
    pub actions: Vec<Action>,
}

#[tauri::command]
pub async fn load_gui_inventory(
    driver: tauri::State<'_, Arc<::driver::Driver>>,
) -> Result<LoadGuiInventoryResult, CommandError> {
    let driver = driver.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let classes = driver
            .list_classes()?
            .into_iter()
            .map(|class_info| (class_info.id.clone(), class_info))
            .collect::<HashMap<_, _>>();
        let inventory = driver
            .sync_inventory(None)
            .unwrap_or_else(|err| {
                eprintln!("zk-craft: failed to sync inventory, falling back to local: {err}");
                driver.list_objects(None).unwrap_or_default()
            })
            .into_iter()
            .map(|object| {
                let class_info = classes.get(&object.class_id);
                InventoryObject {
                    id: object.id,
                    file_name: object.file_name,
                    class_id: object.class_id.clone(),
                    class_display_name: object.class_display_name,
                    plugin_name: object.plugin_name,
                    class_hash: object.class_hash,
                    emoji: class_info
                        .map(|class_info| class_info.emoji.clone())
                        .unwrap_or_else(|| "📦".to_string()),
                    status: object.status,
                    tx_hash: object.tx_hash,
                    grounded: object.grounded.unwrap_or(false),
                    description: class_info.map(|class_info| class_info.description.clone()),
                    obj: serde_json::Value::Object(object.fields.into_iter().collect()),
                }
            })
            .collect();

        let actions = driver
            .list_actions(None)?
            .into_iter()
            .map(|action| Action {
                id: action.id,
                display_name: action.display_name,
                plugin_name: action.plugin_name,
                emoji: action.emoji,
                hash: action.hash,
                total_inputs: action.total_inputs,
                description: action.description,
            })
            .collect();

        Ok::<_, anyhow::Error>(LoadGuiInventoryResult { inventory, actions })
    })
    .await
    .map_err(|err| anyhow::anyhow!("failed to load inventory task: {err}"))?
    .map_err(Into::into)
}

#[tauri::command]
pub async fn get_global_state_root(
    driver: tauri::State<'_, Arc<::driver::Driver>>,
) -> Result<String, CommandError> {
    let driver = driver.inner().clone();
    tauri::async_runtime::spawn_blocking(move || driver.get_state_root())
        .await
        .map_err(|err| anyhow::anyhow!("failed to load state root task: {err}"))?
        .map_err(Into::into)
}
