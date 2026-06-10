//! Tauri shell for the desktop app.
//!
//! This shell holds **no** driver state of its own. The webview talks to dobjd over
//! HTTP just like a browser would. The remaining Tauri commands are desktop-only
//! conveniences that don't need a `Driver` instance:
//!
//! - native file picker for `.dobj` files
//! - in-memory parse of a picked file (for inspection before passing the
//!   filename to a `runAction` HTTP call)
//! - process CPU sample for the desktop app's status bar
//! - native menu (incl. `Cmd+,` settings shortcut)
//!
//! Every call that touches `~/.dobj/` state — objects, run_action, settings,
//! state-root, MCP — lives in [`dobjd`]. The user must
//! start `dobjd` separately for the desktop app's webview to function.

mod cpu;
mod error;
mod objects;
mod settings;

use cpu::{sample_app_cpu, CpuMonitor};
use objects::{open_objects_dir, pick_dobj_file_path, read_dobj_file};
use settings::{build_app_menu, handle_settings_menu_event};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    if let Err(err) = payload::load_dotenv() {
        eprintln!("failed to load gui env: {err}");
    }
    let _ = env_logger::builder().try_init();

    tauri::Builder::default()
        .menu(build_app_menu)
        .on_menu_event(|app, event| {
            handle_settings_menu_event(app, event.id());
        })
        .plugin(tauri_plugin_opener::init())
        .manage(CpuMonitor::new())
        .invoke_handler(tauri::generate_handler![
            sample_app_cpu,
            pick_dobj_file_path,
            read_dobj_file,
            open_objects_dir
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
