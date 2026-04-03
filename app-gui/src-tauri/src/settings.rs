use std::sync::Arc;

use crate::error::CommandError;
use serde::{Deserialize, Serialize};
use tauri::{
    menu::{Menu, MenuItem, MenuItemBuilder},
    AppHandle, Emitter, Runtime,
};

const MENU_OPEN_SETTINGS_ID: &str = "app.open-settings";
const OPEN_SETTINGS_EVENT: &str = "open-settings";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub synchronizer_api_url: String,
    pub relayer_api_url: String,
}

impl From<driver::DriverSettings> for AppSettings {
    fn from(value: driver::DriverSettings) -> Self {
        Self {
            synchronizer_api_url: value.synchronizer_api_url,
            relayer_api_url: value.relayer_api_url,
        }
    }
}

impl From<AppSettings> for driver::DriverSettings {
    fn from(value: AppSettings) -> Self {
        Self {
            synchronizer_api_url: value.synchronizer_api_url,
            relayer_api_url: value.relayer_api_url,
        }
    }
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

#[tauri::command]
pub fn get_app_settings(
    driver: tauri::State<'_, Arc<driver::Driver>>,
) -> Result<AppSettings, CommandError> {
    Ok(driver.load_settings()?.into())
}

#[tauri::command]
pub fn save_app_settings(
    driver: tauri::State<'_, Arc<driver::Driver>>,
    input: AppSettings,
) -> Result<AppSettings, CommandError> {
    Ok(driver.save_settings(&input.clone().into())?.into())
}
