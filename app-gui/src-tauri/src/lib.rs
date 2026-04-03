mod bootstrap;
mod cpu;
mod error;
mod mcp;
mod objects;
mod progress;
mod run_action;
mod settings;

use std::sync::Arc;

use bootstrap::{get_global_state_root, load_gui_inventory};
use cpu::{sample_app_cpu, CpuMonitor};
use objects::{
    get_objects_dir, open_objects_dir, pick_dobj_file_path, read_dobj_file, start_objects_watcher,
};
use run_action::run_action;
use settings::{build_app_menu, get_app_settings, handle_settings_menu_event, save_app_settings};

use craft_mcp::DEFAULT_PORT as MCP_PORT;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    if let Err(err) = common::load_dotenv() {
        eprintln!("zk-craft: failed to load app-gui env: {err}");
    }

    let driver = Arc::new(::driver::Driver::open_default().expect("failed to initialize driver"));

    tauri::Builder::default()
        .menu(build_app_menu)
        .on_menu_event(|app, event| {
            handle_settings_menu_event(app, event.id());
        })
        .plugin(tauri_plugin_opener::init())
        .manage(CpuMonitor::new())
        .manage(Arc::clone(&driver))
        .setup(|app| {
            let driver = tauri::Manager::state::<Arc<::driver::Driver>>(app);
            if let Err(err) =
                start_objects_watcher(app.handle().clone(), driver.paths().objects_dir.clone())
            {
                eprintln!("zk-craft: objects watcher disabled: {err}");
            }

            // Start MCP server
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Err(err) = start_mcp_server(handle).await {
                    eprintln!("zk-craft: MCP server failed: {err}");
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            sample_app_cpu,
            load_gui_inventory,
            get_global_state_root,
            run_action,
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

async fn start_mcp_server(app: tauri::AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let driver = tauri::Manager::state::<Arc<::driver::Driver>>(&app)
        .inner()
        .clone();
    let ops = mcp::AppCraftOps::new(app, driver);
    let config = craft_mcp::McpConfig::default();
    let server = craft_mcp::McpServer::new(ops, config);

    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{MCP_PORT}")).await?;
    eprintln!("zk-craft: MCP server listening on http://127.0.0.1:{MCP_PORT}/mcp");

    server.serve(listener).await?;
    Ok(())
}
