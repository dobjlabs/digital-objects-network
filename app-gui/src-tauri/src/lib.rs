mod cpu;
mod objects;
mod sdk;
mod settings;
mod spec;

use cpu::{sample_app_cpu, CpuMonitor};
use objects::{
    get_objects_dir, open_objects_dir, pick_dobj_file_path, read_dobj_file, start_objects_watcher,
};
use sdk::{get_global_state_root, load_gui_inventory, run_sdk_action, ActionRunGate};
use settings::{build_app_menu, get_app_settings, handle_settings_menu_event, save_app_settings};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    if let Err(err) = common::load_dotenv() {
        eprintln!("zk-craft: failed to load app-gui env: {err}");
    }
    tauri::Builder::default()
        .menu(build_app_menu)
        .on_menu_event(|app, event| {
            handle_settings_menu_event(app, event.id());
        })
        .plugin(tauri_plugin_opener::init())
        .manage(CpuMonitor::new())
        .manage(ActionRunGate::new())
        .setup(|app| {
            if let Err(err) = start_objects_watcher(app.handle().clone()) {
                eprintln!("zk-craft: objects watcher disabled: {err}");
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            sample_app_cpu,
            load_gui_inventory,
            get_global_state_root,
            run_sdk_action,
            get_objects_dir,
            open_objects_dir,
            pick_dobj_file_path,
            read_dobj_file,
            get_app_settings,
            save_app_settings
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
