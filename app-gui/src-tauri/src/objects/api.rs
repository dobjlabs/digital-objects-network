use std::{fs, path::PathBuf};

use crate::error::CommandError;
use crate::objects::objects_dir;
use crate::objects::ObjectRecord;
use crate::sdk::parse_object_file_from_path;
use anyhow::{anyhow, Result};
use rfd::FileDialog;
use tauri_plugin_opener::OpenerExt;

#[tauri::command]
pub fn get_objects_dir(app: tauri::AppHandle) -> Result<String, CommandError> {
    let path = objects_dir(&app)?;
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
pub fn open_objects_dir(app: tauri::AppHandle) -> Result<String, CommandError> {
    let path: PathBuf = objects_dir(&app)?;
    fs::create_dir_all(&path)
        .map_err(|err| anyhow!("failed to create objects directory: {err}"))?;
    app.opener()
        .open_path(path.to_string_lossy().to_string(), None::<&str>)
        .map_err(|err| anyhow!("failed to open objects directory: {err}"))?;
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
pub fn pick_dobj_file_path() -> Result<String, CommandError> {
    let Some(path) = FileDialog::new()
        .add_filter("Digital Object (.dobj)", &["dobj"])
        .pick_file()
    else {
        return Err(anyhow!("No file selected").into());
    };
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
pub fn read_dobj_file(path: String) -> Result<ObjectRecord, CommandError> {
    let path = PathBuf::from(path.trim());
    if !path.exists() {
        return Err(anyhow!("selected file does not exist: {}", path.display()).into());
    }
    Ok(parse_object_file_from_path(&path)?)
}
