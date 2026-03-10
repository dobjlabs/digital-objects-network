mod action_spec;
mod commands;
mod id_codegen;
mod objects_watcher;
mod state;
mod types;

use tauri::{
    menu::{Menu, MenuItem, MenuItemBuilder},
    AppHandle, Emitter, Runtime,
};

use commands::{
    get_app_settings, get_things_dir, load_gui_bootstrap, open_things_dir, pick_dobj_file_path,
    read_dobj_file_metadata, run_sdk_action, sample_app_cpu, save_app_settings,
};
use objects_watcher::start_objects_watcher;
use state::{CpuMonitor, CraftRuntime};

const MENU_OPEN_SETTINGS_ID: &str = "app.open-settings";
const OPEN_SETTINGS_EVENT: &str = "open-settings";

fn build_app_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let menu = Menu::default(app)?;
    inject_settings_menu_item(app, &menu)?;
    Ok(menu)
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    if let Err(err) = common::load_dotenv() {
        eprintln!("zk-craft: failed to load app-gui env: {err}");
    }
    tauri::Builder::default()
        .menu(build_app_menu)
        .on_menu_event(|app, event| {
            if event.id() == MENU_OPEN_SETTINGS_ID {
                let _ = app.emit(OPEN_SETTINGS_EVENT, ());
            }
        })
        .plugin(tauri_plugin_opener::init())
        .manage(CpuMonitor::new())
        .manage(CraftRuntime::new())
        .setup(|app| {
            if let Err(err) = start_objects_watcher(app.handle().clone()) {
                eprintln!("zk-craft: objects watcher disabled: {err}");
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            sample_app_cpu,
            load_gui_bootstrap,
            run_sdk_action,
            get_things_dir,
            open_things_dir,
            pick_dobj_file_path,
            read_dobj_file_metadata,
            get_app_settings,
            save_app_settings
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
