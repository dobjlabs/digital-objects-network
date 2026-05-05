use std::collections::HashMap;

use anyhow::Result;
use axum::{Json, extract::State};
use serde::Serialize;

use crate::error::ApiResult;
use crate::state::AppState;

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct InventoryObject {
    pub id: String,
    pub file_name: String,
    pub class_name: String,
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
    pub emoji: String,
    pub hash: String,
    pub total_input_class_hashes: Vec<String>,
    pub description: String,
    pub total_input_classes: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadGuiInventoryResult {
    pub inventory: Vec<InventoryObject>,
    pub actions: Vec<Action>,
}

pub async fn load_inventory(
    State(state): State<AppState>,
) -> ApiResult<Json<LoadGuiInventoryResult>> {
    let driver = state.driver.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<LoadGuiInventoryResult> {
        let classes = driver
            .list_classes()?
            .into_iter()
            .map(|class_info| (class_info.name.clone(), class_info))
            .collect::<HashMap<_, _>>();

        let inventory = driver
            .sync_inventory(None)
            .unwrap_or_else(|err| {
                eprintln!("dobjd: failed to sync inventory, falling back to local: {err}");
                driver.list_objects(None).unwrap_or_default()
            })
            .into_iter()
            .map(|object| {
                let class_info = classes.get(&object.class_name);
                InventoryObject {
                    id: object.id,
                    file_name: object.file_name,
                    class_name: object.class_name.clone(),
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
                emoji: action.emoji,
                hash: action.hash,
                total_input_class_hashes: action.total_input_class_hashes,
                description: action.description,
                total_input_classes: action.total_input_classes,
            })
            .collect();

        Ok(LoadGuiInventoryResult { inventory, actions })
    })
    .await
    .map_err(|err| anyhow::anyhow!("inventory task panicked: {err}"))??;

    Ok(Json(result))
}
