use anyhow::{anyhow, Result};
use std::{fs, path::PathBuf};

use crate::error::CommandError;
use serde::{Deserialize, Serialize};
use tauri::{
    menu::{Menu, MenuItem, MenuItemBuilder},
    AppHandle, Emitter, Manager, Runtime,
};

const MENU_OPEN_SETTINGS_ID: &str = "app.open-settings";
const OPEN_SETTINGS_EVENT: &str = "open-settings";
const DEFAULT_SYNCHRONIZER_API_URL: &str = "http://127.0.0.1:3000";
const DEFAULT_RELAYER_API_URL: &str = "http://127.0.0.1:3200";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub synchronizer_api_url: String,
    pub relayer_api_url: String,
}

pub(crate) fn build_app_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let menu = Menu::default(app)?;
    inject_settings_menu_item(app, &menu)?;
    Ok(menu)
}

pub(crate) fn handle_settings_menu_event<R: Runtime>(app: &AppHandle<R>, menu_id: impl AsRef<str>) {
    if menu_id.as_ref() == MENU_OPEN_SETTINGS_ID {
        let _ = app.emit(OPEN_SETTINGS_EVENT, ());
    }
}

fn inject_settings_menu_item<R: Runtime>(app: &AppHandle<R>, menu: &Menu<R>) -> tauri::Result<()> {
    let settings_item = MenuItemBuilder::with_id(MENU_OPEN_SETTINGS_ID, "Settings...")
        .accelerator("CmdOrCtrl+,")
        .build(app)?;

    #[cfg(target_os = "macos")]
    if insert_settings_into_first_submenu(menu, &settings_item)? {
        return Ok(());
    }

    let _ = append_settings_to_named_submenu(menu, "File", &settings_item)?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn insert_settings_into_first_submenu<R: Runtime>(
    menu: &Menu<R>,
    settings_item: &MenuItem<R>,
) -> tauri::Result<bool> {
    for item in menu.items()? {
        if let Some(submenu) = item.as_submenu() {
            submenu.insert(settings_item, 1)?;
            return Ok(true);
        }
    }
    Ok(false)
}

fn append_settings_to_named_submenu<R: Runtime>(
    menu: &Menu<R>,
    submenu_name: &str,
    settings_item: &MenuItem<R>,
) -> tauri::Result<bool> {
    for item in menu.items()? {
        let Some(submenu) = item.as_submenu() else {
            continue;
        };
        if submenu.text()? == submenu_name {
            submenu.append(settings_item)?;
            return Ok(true);
        }
    }
    Ok(false)
}

fn default_settings() -> AppSettings {
    AppSettings {
        synchronizer_api_url: option_env!("DEFAULT_SYNCHRONIZER_API_URL")
            .unwrap_or(DEFAULT_SYNCHRONIZER_API_URL)
            .to_string(),
        relayer_api_url: option_env!("DEFAULT_RELAYER_API_URL")
            .unwrap_or(DEFAULT_RELAYER_API_URL)
            .to_string(),
    }
}

fn settings_path(app: &tauri::AppHandle) -> Result<PathBuf> {
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|err| anyhow!("failed to resolve app config directory: {err}"))?;
    fs::create_dir_all(&config_dir)
        .map_err(|err| anyhow!("failed to create app config directory: {err}"))?;
    Ok(config_dir.join("settings.json"))
}

fn read_settings(app: &tauri::AppHandle) -> Result<Option<AppSettings>> {
    let path = settings_path(app)?;
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path)
        .map_err(|err| anyhow!("failed to read settings file {}: {err}", path.display()))?;
    let settings = serde_json::from_str::<AppSettings>(&raw)
        .map_err(|err| anyhow!("failed to parse settings file {}: {err}", path.display()))?;
    Ok(Some(settings))
}

fn write_settings(app: &tauri::AppHandle, settings: &AppSettings) -> Result<()> {
    let path = settings_path(app)?;
    let serialized = serde_json::to_string(settings)
        .map_err(|err| anyhow!("failed to serialize settings: {err}"))?;
    fs::write(&path, serialized)
        .map_err(|err| anyhow!("failed to write settings file {}: {err}", path.display()))
}

#[tauri::command]
pub fn get_app_settings(app: tauri::AppHandle) -> Result<AppSettings, CommandError> {
    if let Some(settings) = read_settings(&app)? {
        return Ok(settings);
    }
    let defaults = default_settings();
    write_settings(&app, &defaults)?;
    Ok(defaults)
}

#[tauri::command]
pub fn save_app_settings(
    app: tauri::AppHandle,
    input: AppSettings,
) -> Result<AppSettings, CommandError> {
    write_settings(&app, &input)?;
    Ok(input)
}
