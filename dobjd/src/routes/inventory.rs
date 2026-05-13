use std::collections::HashMap;

use anyhow::Result;
use axum::{Json, extract::State};
use wire_types::InventoryObject;

use crate::error::ApiResult;
use crate::state::AppState;

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
            .map(|class_info| (class_info.class.clone(), class_info))
            .collect::<HashMap<_, _>>();

        Ok(driver
            .sync_inventory(None)
            .unwrap_or_else(|err| {
                tracing::warn!("sync_inventory failed, falling back to local: {err:#}");
                driver.list_objects(None).unwrap_or_default()
            })
            .into_iter()
            .map(|object| {
                let class_info = classes.get(&object.class);
                InventoryObject {
                    id: object.id,
                    file_name: object.file_name,
                    class: object.class.clone(),
                    class_hash: object.class_hash,
                    emoji: class_info
                        .map(|class_info| class_info.emoji.clone())
                        .unwrap_or_else(|| "📦".to_string()),
                    status: object.status,
                    tx_hash: object.tx_hash,
                    description: class_info.map(|class_info| class_info.description.clone()),
                    fields: object.fields,
                }
            })
            .collect())
    })
    .await
    .map_err(|err| anyhow::anyhow!("inventory task panicked: {err}"))??;

    Ok(Json(inventory))
}
