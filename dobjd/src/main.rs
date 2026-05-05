use std::sync::Arc;

use anyhow::Result;
use driver::Driver;

mod error;
mod events;
mod mcp;
mod progress;
mod routes;
mod state;
mod watcher;

use state::AppState;

const DEFAULT_PORT: u16 = 7717;

#[tokio::main]
async fn main() -> Result<()> {
    if let Err(err) = common::load_dotenv() {
        eprintln!("dobjd: failed to load env: {err}");
    }
    let _ = env_logger::builder().try_init();

    let port = std::env::var("DOBJD_PORT")
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(DEFAULT_PORT);

    let static_dir = std::env::var("DOBJD_STATIC_DIR")
        .ok()
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from);

    let driver = Arc::new(Driver::open_default()?);
    let (event_tx, _initial_rx) = events::channel();

    if let Err(err) =
        watcher::start_objects_watcher(event_tx.clone(), driver.paths().objects_dir.clone())
    {
        eprintln!("dobjd: objects watcher disabled: {err}");
    }

    let state = AppState::new(driver.clone(), event_tx.clone());
    if let Some(ref dir) = static_dir {
        eprintln!("dobjd: serving static frontend from {}", dir.display());
    }
    let app = routes::router(state, static_dir);

    // Spawn the MCP server alongside the HTTP API. Both share the same
    // `Arc<Driver>` and broadcast hub, so an MCP client and the desktop /
    // website end up driving one process.
    let mcp_event_tx = event_tx.clone();
    let mcp_driver = driver.clone();
    tokio::spawn(async move {
        if let Err(err) = start_mcp_server(mcp_driver, mcp_event_tx).await {
            eprintln!("dobjd: MCP server failed: {err}");
        }
    });

    let addr = format!("127.0.0.1:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    eprintln!("dobjd: listening on http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn start_mcp_server(driver: Arc<Driver>, events: events::EventTx) -> Result<()> {
    let ops = mcp::DobjdCraftOps::new(driver, events);
    let config = craft_mcp::McpConfig::default();
    let server = craft_mcp::McpServer::new(ops, config);

    let port = craft_mcp::DEFAULT_PORT;
    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}")).await?;
    eprintln!("dobjd: MCP server listening on http://127.0.0.1:{port}/mcp");
    server.serve(listener).await?;
    Ok(())
}
