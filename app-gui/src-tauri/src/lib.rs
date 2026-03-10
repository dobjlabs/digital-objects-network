mod commands;
mod objects_watcher;
mod state;
mod types;

use commands::{
    get_things_dir, load_gui_bootstrap, open_things_dir, run_sdk_action, sample_app_cpu,
};
use objects_watcher::start_objects_watcher;
use state::{CpuMonitor, CraftRuntime};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    if let Err(err) = common::load_dotenv() {
        eprintln!("zk-craft: failed to load app-gui env: {err}");
    }
    tauri::Builder::default()
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
            open_things_dir
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
