use std::{fs, path::PathBuf};

use rfd::FileDialog;
use serde::{Deserialize, Serialize};
use tauri::Manager;
use tauri_plugin_opener::OpenerExt;

fn resolve_objects_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let home = app
        .path()
        .home_dir()
        .map_err(|err| format!("failed to resolve home directory: {err}"))?;
    Ok(home.join(".objects"))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedObjectFile {
    file_name: Option<String>,
    class_name: Option<String>,
    validity: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DobjFileMetadata {
    pub file_name: String,
    pub class_name: String,
    pub validity: String,
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
    let parsed = serde_json::from_str::<PersistedObjectFile>(&contents)
        .map_err(|err| format!("invalid .dobj JSON in {}: {err}", path.display()))?;

    let file_name = parsed
        .file_name
        .and_then(|name| {
            let trimmed = name.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        })
        .unwrap_or(file_name_from_path);
    let class_name = parsed
        .class_name
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| format!("missing className in {}", path.display()))?;
    let validity = parsed
        .validity
        .map(|value| value.trim().to_lowercase())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("missing validity in {}", path.display()))?;
    if validity != "live" && validity != "nullified" {
        return Err(format!(
            "invalid validity '{}' in {} (expected 'live' or 'nullified')",
            validity,
            path.display()
        ));
    }

    Ok(DobjFileMetadata {
        file_name,
        class_name,
        validity,
    })
}
