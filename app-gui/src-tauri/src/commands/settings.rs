use std::{env, fs, path::PathBuf};

use serde::{Deserialize, Serialize};
use tauri::Manager;

const DEFAULT_SYNCHRONIZER_API_URL: &str = "http://127.0.0.1:3000";
const DEFAULT_RELAYER_API_URL: &str = "http://127.0.0.1:3200";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettingsDto {
    pub synchronizer_api_url: String,
    pub relayer_api_url: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveAppSettingsInput {
    pub synchronizer_api_url: String,
    pub relayer_api_url: String,
}

#[derive(Debug, Deserialize, Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct PersistedAppSettings {
    synchronizer_api_url: Option<String>,
    relayer_api_url: Option<String>,
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

fn default_from_env(name: &str, fallback: &str) -> String {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

fn read_persisted_settings(app: &tauri::AppHandle) -> Result<PersistedAppSettings, String> {
    let path = settings_path(app)?;
    if !path.exists() {
        return Ok(PersistedAppSettings::default());
    }
    let raw = fs::read_to_string(&path)
        .map_err(|err| format!("failed to read settings file {}: {err}", path.display()))?;
    serde_json::from_str::<PersistedAppSettings>(&raw)
        .map_err(|err| format!("failed to parse settings file {}: {err}", path.display()))
}

fn write_persisted_settings(
    app: &tauri::AppHandle,
    settings: &PersistedAppSettings,
) -> Result<(), String> {
    let path = settings_path(app)?;
    let serialized = serde_json::to_string_pretty(settings)
        .map_err(|err| format!("failed to serialize settings: {err}"))?;
    fs::write(&path, serialized)
        .map_err(|err| format!("failed to write settings file {}: {err}", path.display()))
}

fn non_empty_or_none(value: String) -> Option<String> {
    let trimmed = value.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn to_effective_settings(
    persisted: PersistedAppSettings,
    sync_default: String,
    relayer_default: String,
) -> AppSettingsDto {
    AppSettingsDto {
        synchronizer_api_url: persisted.synchronizer_api_url.unwrap_or(sync_default),
        relayer_api_url: persisted.relayer_api_url.unwrap_or(relayer_default),
    }
}

pub(crate) fn load_effective_endpoint_urls(app: &tauri::AppHandle) -> Result<AppSettingsDto, String> {
    let persisted = read_persisted_settings(app)?;
    let sync_default =
        default_from_env("SYNCHRONIZER_API_URL", DEFAULT_SYNCHRONIZER_API_URL);
    let relayer_default = default_from_env("RELAYER_API_URL", DEFAULT_RELAYER_API_URL);
    Ok(to_effective_settings(
        persisted,
        sync_default,
        relayer_default,
    ))
}

#[tauri::command]
pub fn get_app_settings(app: tauri::AppHandle) -> Result<AppSettingsDto, String> {
    load_effective_endpoint_urls(&app)
}

#[tauri::command]
pub fn save_app_settings(
    app: tauri::AppHandle,
    input: SaveAppSettingsInput,
) -> Result<AppSettingsDto, String> {
    let persisted = PersistedAppSettings {
        synchronizer_api_url: non_empty_or_none(input.synchronizer_api_url),
        relayer_api_url: non_empty_or_none(input.relayer_api_url),
    };
    write_persisted_settings(&app, &persisted)?;
    load_effective_endpoint_urls(&app)
}
