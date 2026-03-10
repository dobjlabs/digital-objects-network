use std::fs;
use std::path::PathBuf;
use tauri::Manager;
use tauri_plugin_opener::OpenerExt;

fn resolve_objects_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let home = app
        .path()
        .home_dir()
        .map_err(|err| format!("failed to resolve home directory: {err}"))?;
    Ok(home.join(".objects"))
}

#[tauri::command]
pub fn get_things_dir(app: tauri::AppHandle) -> Result<String, String> {
    let path = resolve_objects_dir(&app)?;
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
pub fn open_things_dir(app: tauri::AppHandle) -> Result<String, String> {
    let path = resolve_objects_dir(&app)?;
    fs::create_dir_all(&path)
        .map_err(|err| format!("failed to create objects directory: {err}"))?;
    app.opener()
        .open_path(path.to_string_lossy().to_string(), None::<&str>)
        .map_err(|err| format!("failed to open objects directory: {err}"))?;
    Ok(path.to_string_lossy().to_string())
}
