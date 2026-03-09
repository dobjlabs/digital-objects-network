mod commands;
mod state;
mod types;

use commands::{create_dobj, get_things_dir, open_things_dir, sample_app_cpu};
use state::CpuMonitor;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(CpuMonitor::new())
        .invoke_handler(tauri::generate_handler![
            sample_app_cpu,
            create_dobj,
            get_things_dir,
            open_things_dir
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
