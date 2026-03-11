use super::{
    mapping::{build_action_catalog, to_inventory_item},
    object_store::sync_object_files,
    runtime::{empty_state_root, ensure_runtime_loaded, lock_runtime, refresh_runtime_objects},
    synchronizer_client::fetch_synchronizer_head,
};
use crate::{app_paths, state::ObjectsRuntime, types::LoadGuiBootstrapResult};

use super::super::settings::get_app_settings;

#[tauri::command]
pub async fn load_gui_bootstrap(
    app: tauri::AppHandle,
    runtime: tauri::State<'_, ObjectsRuntime>,
) -> Result<LoadGuiBootstrapResult, String> {
    let objects_dir = app_paths::objects_dir(&app)?;
    let actions = build_action_catalog();
    let effective_urls = get_app_settings(app.clone())?;
    let sync_head = fetch_synchronizer_head(&effective_urls.synchronizer_api_url);

    let mut inner = lock_runtime(&runtime);
    if let Err(err) = ensure_runtime_loaded(&mut inner, &objects_dir) {
        eprintln!("zk-craft: bootstrap runtime failed, resetting state: {err}");
        inner.state_root = empty_state_root();
        inner.objects.clear();
        inner.loaded = true;
        let _ = sync_object_files(&inner, &objects_dir);
    }
    if !inner.run_in_progress {
        if let Err(err) = refresh_runtime_objects(&mut inner, &objects_dir) {
            eprintln!("zk-craft: failed to refresh objects from disk: {err}");
        }
    }
    if let Err(err) = sync_head {
        eprintln!("zk-craft: synchronizer unavailable during bootstrap: {err}");
    }

    Ok(LoadGuiBootstrapResult {
        objects: inner.objects.iter().map(to_inventory_item).collect(),
        actions,
    })
}
