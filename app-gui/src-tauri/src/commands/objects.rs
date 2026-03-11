use std::{fs, path::PathBuf};

use crate::app_paths;
use rfd::FileDialog;
use serde::{Deserialize, Serialize};
use tauri_plugin_opener::OpenerExt;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DobjFileMetadata {
    #[serde(default)]
    pub file_name: String,
    pub class_name: String,
    pub validity: String,
}

#[tauri::command]
pub fn get_objects_dir(app: tauri::AppHandle) -> Result<String, String> {
    let path = app_paths::objects_dir(&app)?;
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
pub fn open_objects_dir(app: tauri::AppHandle) -> Result<String, String> {
    let path = app_paths::objects_dir(&app)?;
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
pub fn read_dobj_file_metadata(path: String) -> Result<DobjFileMetadata, String> {
    let path = PathBuf::from(path.trim());
    if !path.exists() {
        return Err(format!("selected file does not exist: {}", path.display()));
    }
    let file_name_from_path = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("selected.dobj")
        .to_string();
    let contents = fs::read_to_string(&path)
        .map_err(|err| format!("failed to read selected file {}: {err}", path.display()))?;
    let mut parsed = serde_json::from_str::<DobjFileMetadata>(&contents)
        .map_err(|err| format!("invalid .dobj JSON in {}: {err}", path.display()))?;

    parsed.file_name = {
        let trimmed = parsed.file_name.trim().to_string();
        if trimmed.is_empty() {
            file_name_from_path
        } else {
            trimmed
        }
    };
    parsed.class_name = parsed.class_name.trim().to_string();
    if parsed.class_name.is_empty() {
        return Err(format!("missing className in {}", path.display()));
    }
    parsed.validity = parsed.validity.trim().to_lowercase();
    if parsed.validity.is_empty() {
        return Err(format!("missing validity in {}", path.display()));
    }
    if parsed.validity != "live" && parsed.validity != "nullified" {
        return Err(format!(
            "invalid validity '{}' in {} (expected 'live' or 'nullified')",
            parsed.validity,
            path.display()
        ));
    }

    Ok(parsed)
}
