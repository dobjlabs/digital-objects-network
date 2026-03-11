mod app_paths;
mod commands;
mod cpu;
mod objects;
mod sdk;
mod spec;

use commands::{
    build_app_menu, get_app_settings, get_objects_dir, handle_settings_menu_event,
    load_gui_bootstrap, open_objects_dir, pick_dobj_file_path, read_dobj_file, run_sdk_action,
    sample_app_cpu, save_app_settings, ActionRunGate,
};
use cpu::CpuMonitor;
use objects::start_objects_watcher;

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
            load_gui_bootstrap,
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
