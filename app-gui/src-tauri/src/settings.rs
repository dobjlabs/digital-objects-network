//! Settings menu (Cmd+,) plus the `get_app_settings` / `save_app_settings`
//! Tauri commands.
//!
//! Saving writes the new settings file to disk. The in-memory `Driver` keeps
//! the values it loaded at startup until the next launch — restart the app
//! to pick up changes. (The risc0 stack doesn't need hot-swappable
//! synchronizer / relayer URLs in normal use.)

use std::sync::Arc;

use crate::error::CommandError;
use tauri::{
    AppHandle, Emitter, Runtime,
    menu::{Menu, MenuItem, MenuItemBuilder},
};

const MENU_OPEN_SETTINGS_ID: &str = "app.open-settings";
const OPEN_SETTINGS_EVENT: &str = "open-settings";

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
    driver: tauri::State<'_, Arc<::driver::Driver>>,
) -> Result<::driver::DriverSettings, CommandError> {
    Ok(driver.settings.clone())
}

#[tauri::command]
pub fn save_app_settings(
    driver: tauri::State<'_, Arc<::driver::Driver>>,
    input: ::driver::DriverSettings,
) -> Result<::driver::DriverSettings, CommandError> {
    ::driver::settings::write_settings(&driver.paths, &input).map_err(anyhow::Error::from)?;
    Ok(input)
}
