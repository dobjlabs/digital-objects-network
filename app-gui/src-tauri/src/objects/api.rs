use std::{fs, path::PathBuf, sync::Arc};

use crate::error::CommandError;
use anyhow::{Result, anyhow};
use rfd::FileDialog;
use tauri_plugin_opener::OpenerExt;

#[tauri::command]
pub fn get_objects_dir(
    driver: tauri::State<'_, Arc<::driver::Driver>>,
) -> Result<String, CommandError> {
    Ok(driver.paths.objects_dir.to_string_lossy().to_string())
}

#[tauri::command]
pub fn open_objects_dir(
    app: tauri::AppHandle,
    driver: tauri::State<'_, Arc<::driver::Driver>>,
) -> Result<String, CommandError> {
    let path: PathBuf = driver.paths.objects_dir.clone();
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
        .add_filter("Digital Object (.dobj)", &[::driver::paths::DOBJ_EXTENSION])
        .pick_file()
    else {
        return Err(anyhow!("No file selected").into());
    };
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
pub fn read_dobj_file(path: String) -> Result<::driver::ObjectRecord, CommandError> {
    let path = PathBuf::from(path.trim());
    if !path.exists() {
        return Err(anyhow!("selected file does not exist: {}", path.display()).into());
    }
    Ok(::driver::object::parse_object_record_file(&path)?)
}
