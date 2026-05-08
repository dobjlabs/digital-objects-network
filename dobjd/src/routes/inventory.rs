use std::collections::HashMap;

use anyhow::Result;
use axum::{Json, extract::State};
use serde::Serialize;

use crate::error::ApiResult;
use crate::state::AppState;

/// Inventory object with class metadata folded in (emoji, description) so
/// GUI clients can render rows without a second `/classes` round-trip.
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
    pub description: Option<String>,
    pub obj: serde_json::Value,
}

/// `GET /inventory` — local objects synced against the chain. The action
/// catalog comes from `GET /actions` separately, so clients can fetch the
/// two in parallel.
pub async fn load_inventory(
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<InventoryObject>>> {
    let driver = state.driver.clone();
    let inventory = tokio::task::spawn_blocking(move || -> Result<Vec<InventoryObject>> {
        let classes = driver
            .list_classes()?
            .into_iter()
            .map(|class_info| (class_info.name.clone(), class_info))
            .collect::<HashMap<_, _>>();

        Ok(driver
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
                    description: class_info.map(|class_info| class_info.description.clone()),
                    obj: serde_json::Value::Object(object.fields.into_iter().collect()),
                }
            })
            .collect())
    })
    .await
    .map_err(|err| anyhow::anyhow!("inventory task panicked: {err}"))??;

    Ok(Json(inventory))
}
