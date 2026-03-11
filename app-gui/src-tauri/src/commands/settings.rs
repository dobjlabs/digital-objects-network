use std::{fs, path::PathBuf};

use serde::{Deserialize, Serialize};
use tauri::Manager;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub synchronizer_api_url: String,
    pub relayer_api_url: String,
}

fn default_settings() -> AppSettings {
    AppSettings {
        synchronizer_api_url: "http://127.0.0.1:3000".to_string(),
        relayer_api_url: "http://127.0.0.1:3200".to_string(),
    }
}

fn settings_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|err| format!("failed to resolve app config directory: {err}"))?;
    fs::create_dir_all(&config_dir)
        .map_err(|err| format!("failed to create app config directory: {err}"))?;
    Ok(config_dir.join("settings.json"))
}

fn read_settings(app: &tauri::AppHandle) -> Result<Option<AppSettings>, String> {
    let path = settings_path(app)?;
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path)
        .map_err(|err| format!("failed to read settings file {}: {err}", path.display()))?;
    let settings = serde_json::from_str::<AppSettings>(&raw)
        .map_err(|err| format!("failed to parse settings file {}: {err}", path.display()))?;
    Ok(Some(settings))
}

fn write_settings(app: &tauri::AppHandle, settings: &AppSettings) -> Result<(), String> {
    let path = settings_path(app)?;
    let serialized = serde_json::to_string_pretty(settings)
        .map_err(|err| format!("failed to serialize settings: {err}"))?;
    fs::write(&path, serialized)
        .map_err(|err| format!("failed to write settings file {}: {err}", path.display()))
}

#[tauri::command]
pub fn get_app_settings(app: tauri::AppHandle) -> Result<AppSettings, String> {
    if let Some(settings) = read_settings(&app)? {
        return Ok(settings);
    }
    let defaults = default_settings();
    write_settings(&app, &defaults)?;
    Ok(defaults)
}

#[tauri::command]
pub fn save_app_settings(app: tauri::AppHandle, input: AppSettings) -> Result<AppSettings, String> {
    write_settings(&app, &input)?;
    Ok(input)
}
