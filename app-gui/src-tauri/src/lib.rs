mod commands;
mod state;
mod types;

use commands::{
    attach_claim, create_post, ensure_things_dir, get_mock_state, get_things_dir, open_things_dir,
    respond_post, run_method, sample_app_cpu, verify_post_proofs,
};
use state::{AppState, CpuMonitor};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::default())
        .manage(CpuMonitor::new())
        .invoke_handler(tauri::generate_handler![
            get_things_dir,
            ensure_things_dir,
            open_things_dir,
            get_mock_state,
            sample_app_cpu,
            run_method,
            verify_post_proofs,
            create_post,
            respond_post,
            attach_claim
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
