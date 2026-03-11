use super::{
    mapping::{build_action_catalog, to_inventory_item, InventoryItemDto, RecipeDto},
    object_store::{ensure_objects_dirs, load_object_files},
    synchronizer_client::fetch_synchronizer_head,
};
use crate::app_paths;
use serde::Serialize;

use super::super::settings::get_app_settings;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadGuiBootstrapResult {
    pub objects: Vec<InventoryItemDto>,
    pub actions: Vec<RecipeDto>,
}

#[tauri::command]
pub async fn load_gui_bootstrap(app: tauri::AppHandle) -> Result<LoadGuiBootstrapResult, String> {
    let objects_dir = app_paths::objects_dir(&app)?;
    ensure_objects_dirs(&objects_dir)?;
    let objects = load_object_files(&objects_dir)?;
    let actions = build_action_catalog();
    let app_settings = get_app_settings(app.clone())?;
    let sync_head = fetch_synchronizer_head(&app_settings.synchronizer_api_url);

    if let Err(err) = sync_head {
        eprintln!("zk-craft: synchronizer unavailable during bootstrap: {err}");
    }

    Ok(LoadGuiBootstrapResult {
        objects: objects.iter().map(to_inventory_item).collect(),
        actions,
    })
}
