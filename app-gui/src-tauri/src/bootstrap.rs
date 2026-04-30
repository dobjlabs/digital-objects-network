//! Bootstrap-time data the GUI needs at first paint:
//! - Object inventory (local `.dobj` files)
//! - Action catalog (visible craft-basics actions)
//! - Synchronizer state-root snapshot

use std::collections::HashMap;
use std::sync::Arc;

use serde::Serialize;

use crate::error::CommandError;

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct InventoryObject {
    /// `0x`-prefixed commitment hex.
    pub id: String,
    /// `<class>_0x<hex>.dobj`.
    pub file_name: String,
    pub class_name: String,
    /// Identifier for the class. Post-migration there's no separate per-class
    /// hash (classes are just strings in the catalog), so we surface the
    /// class name itself — the frontend treats this as an opaque chip label.
    pub class_hash: String,
    pub emoji: String,
    pub status: ::driver::ObjectStatus,
    pub tx_hash: Option<String>,
    /// `true` once the object's tx_final has been observed by the
    /// synchronizer (= `status == Live`). Kept for frontend compatibility
    /// with the pod2-era inventory shape.
    pub grounded: bool,
    pub description: Option<String>,
    /// The object's user-visible fields (from `obj.fields`).
    pub obj: serde_json::Value,
}

/// Frontend-facing action shape. `id` is the action name (e.g. "FindLog")
/// — the frontend uses it as a string identifier and passes it back as
/// `RunActionInput.action_id`. The numeric dispatcher key from
/// `craft_actions` lives only on the Rust side.
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Action {
    pub id: String,
    pub emoji: String,
    /// Same comment as `InventoryObject.class_hash` — class identity is
    /// just the name now.
    pub hash: String,
    pub description: String,
    pub input_classes: Vec<String>,
    pub input_class_hashes: Vec<String>,
    pub output_classes: Vec<String>,
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
        let classes: HashMap<&str, &::driver::ClassInfo> = ::driver::all_classes()
            .iter()
            .map(|c| (c.name, c))
            .collect();

        let inventory: Vec<InventoryObject> = driver
            .list_objects()?
            .into_iter()
            .map(|object| {
                let class_info = classes.get(object.class_name.as_str()).copied();
                let file_name =
                    ::driver::object::file_name_for(&object.class_name, object.commitment());
                let obj_json = serde_json::to_value(&object.obj.fields)
                    .unwrap_or(serde_json::Value::Null);
                let grounded = object.status == ::driver::ObjectStatus::Live;
                InventoryObject {
                    id: object.id.clone(),
                    file_name,
                    class_hash: object.class_name.clone(),
                    class_name: object.class_name.clone(),
                    emoji: class_info
                        .map(|c| c.emoji.to_string())
                        .unwrap_or_else(|| "📦".to_string()),
                    status: object.status,
                    tx_hash: object.tx_hash.clone(),
                    grounded,
                    description: class_info.map(|c| c.description.to_string()),
                    obj: obj_json,
                }
            })
            .collect();

        let actions: Vec<Action> = ::driver::all_actions()
            .iter()
            .filter(|a| !a.hidden)
            .map(|a| Action {
                id: a.name.to_string(),
                emoji: a.emoji.to_string(),
                hash: a.name.to_string(),
                description: a.description.to_string(),
                input_classes: a.inputs.iter().map(|s| s.to_string()).collect(),
                input_class_hashes: a.inputs.iter().map(|s| s.to_string()).collect(),
                output_classes: a.outputs.iter().map(|s| s.to_string()).collect(),
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
    tauri::async_runtime::spawn_blocking(move || {
        let head = driver.deps.synchronizer.state_head()?;
        Ok::<_, anyhow::Error>(
            head.current_gsr
                .map(|h| format!("{h}"))
                .unwrap_or_else(|| "none".to_string()),
        )
    })
    .await
    .map_err(|err| anyhow::anyhow!("failed to load state root task: {err}"))?
    .map_err(Into::into)
}
