use std::{fs, path::PathBuf};

use crate::app_paths;
use crate::objects::ObjectRecord;
use crate::sdk::parse_object_file_from_path;
use rfd::FileDialog;
use tauri_plugin_opener::OpenerExt;

#[tauri::command]
pub fn get_objects_dir(app: tauri::AppHandle) -> Result<String, String> {
    let path = app_paths::objects_dir(&app)?;
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
pub fn open_objects_dir(app: tauri::AppHandle) -> Result<String, String> {
    let path: PathBuf = app_paths::objects_dir(&app)?;
    fs::create_dir_all(&path)
        .map_err(|err| format!("failed to create objects directory: {err}"))?;
    app.opener()
        .open_path(path.to_string_lossy().to_string(), None::<&str>)
        .map_err(|err| format!("failed to open objects directory: {err}"))?;
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
pub fn pick_dobj_file_path() -> Result<String, String> {
    let Some(path) = FileDialog::new()
        .add_filter("Digital Object (.dobj)", &["dobj"])
        .pick_file()
    else {
        return Err("No file selected".to_string());
    };
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
pub fn read_dobj_file(path: String) -> Result<ObjectRecord, String> {
    let path = PathBuf::from(path.trim());
    if !path.exists() {
        return Err(format!("selected file does not exist: {}", path.display()));
    }
    Ok(parse_object_file_from_path(&path)?)
}
