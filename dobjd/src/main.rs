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

    let driver = Arc::new(Driver::open_default()?);
    let (event_tx, _initial_rx) = events::channel();

    if let Err(err) =
        watcher::start_objects_watcher(event_tx.clone(), driver.paths().objects_dir.clone())
    {
        eprintln!("dobjd: objects watcher disabled: {err}");
    }

    let state = AppState::new(driver.clone(), event_tx.clone());
    let app = routes::router(state);

    // Bind both listeners up-front so we fail fast and synchronously if
    // either port is taken. Without this, an MCP bind failure would surface
    // asynchronously (or get lost in a spawned task) while the HTTP side
    // looks healthy — and a half-running dobjd is worse than no dobjd,
    // because every other client assumes MCP is reachable on :7718.
    let addr = format!("127.0.0.1:{port}");
    let http_listener = tokio::net::TcpListener::bind(&addr).await?;

    let mcp_addr = format!("127.0.0.1:{}", craft_mcp::DEFAULT_PORT);
    let mcp_listener = tokio::net::TcpListener::bind(&mcp_addr).await?;

    // Both ports are ours. Spawn the MCP server; share `Arc<Driver>` and the
    // broadcast hub so MCP, the desktop, and the website drive one process.
    let mcp_event_tx = event_tx.clone();
    let mcp_driver = driver.clone();
    tokio::spawn(async move {
        if let Err(err) = start_mcp_server(mcp_driver, mcp_event_tx, mcp_listener).await {
            eprintln!("dobjd: MCP server crashed after startup: {err}");
            std::process::exit(1);
        }
    });
    eprintln!("dobjd: MCP server listening on http://{mcp_addr}/mcp");

    eprintln!("dobjd: listening on http://{addr}");
    axum::serve(http_listener, app).await?;
    Ok(())
}

async fn start_mcp_server(
    driver: Arc<Driver>,
    events: events::EventTx,
    listener: tokio::net::TcpListener,
) -> Result<()> {
    let ops = mcp::DobjdCraftOps::new(driver, events);
    let config = craft_mcp::McpConfig::default();
    let server = craft_mcp::McpServer::new(ops, config);
    server.serve(listener).await?;
    Ok(())
}
