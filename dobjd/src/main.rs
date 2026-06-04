use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use driver::Driver;
use tracing_subscriber::{EnvFilter, prelude::*};

mod error;
mod events;
mod mcp;
mod progress;
mod routes;
mod state;

use state::AppState;

const DEFAULT_HTTP_PORT: u16 = 7717;

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    if let Err(err) = common::load_dotenv() {
        tracing::warn!("failed to load env: {err}");
    }

    let port = http_port_from_env()?;
    let mcp_port = mcp_port_for_http_port(port)?;

    let driver = Arc::new(Driver::open_default()?);
    let (event_tx, _initial_rx) = events::channel();

    let state = AppState::new(driver.clone(), event_tx.clone());
    let app = routes::router(state);

    // Bind both listeners up-front so we fail fast and synchronously if
    // either port is taken. Without this, an MCP bind failure would surface
    // asynchronously (or get lost in a spawned task) while the HTTP side
    // looks healthy — and a half-running dobjd is worse than no dobjd,
    // because every other client expects MCP on the adjacent port.
    let addr = format!("127.0.0.1:{port}");
    let http_listener = tokio::net::TcpListener::bind(&addr).await?;

    let mcp_addr = format!("127.0.0.1:{mcp_port}");
    let mcp_listener = tokio::net::TcpListener::bind(&mcp_addr).await?;

    // Build the proving circuits (recursive MainPod, empty pod, and the VDF +
    // lt_eq_u256 intro pods) before accepting requests, so the first action
    // doesn't pay the one-time build. Ports are already bound, so a port
    // conflict still fails fast ahead of this. On a warm cache it's mostly fast
    // reads. Runs on the blocking pool since circuit construction is CPU-bound
    // and synchronous. A failure is fatal: a circuit that can't build now would
    // fail every action, so refuse to start rather than serve a daemon that
    // cannot prove. The `??` propagates both a panic (JoinError) and the
    // warm-up's own error.
    tokio::task::spawn_blocking(driver::warm_proving_circuits)
        .await
        .map_err(|err| anyhow!("circuit warm-up task panicked: {err}"))??;

    // Both ports are ours. Spawn the MCP server; share `Arc<Driver>` and the
    // broadcast hub so MCP, the desktop, and the website drive one process.
    let mcp_event_tx = event_tx.clone();
    let mcp_driver = driver.clone();
    tokio::spawn(async move {
        if let Err(err) = start_mcp_server(mcp_driver, mcp_event_tx, mcp_listener).await {
            tracing::error!("MCP server crashed after startup: {err:#}");
            std::process::exit(1);
        }
    });
    tracing::info!("MCP server listening on http://{mcp_addr}/mcp");

    tracing::info!("listening on http://{addr}");
    axum::serve(http_listener, app).await?;
    Ok(())
}

fn http_port_from_env() -> Result<u16> {
    match std::env::var("DOBJD_PORT") {
        Ok(value) => value
            .parse::<u16>()
            .with_context(|| format!("invalid DOBJD_PORT={value:?}")),
        Err(std::env::VarError::NotPresent) => Ok(DEFAULT_HTTP_PORT),
        Err(err) => Err(anyhow!("invalid DOBJD_PORT env: {err}")),
    }
}

fn mcp_port_for_http_port(port: u16) -> Result<u16> {
    port.checked_add(1)
        .ok_or_else(|| anyhow!("DOBJD_PORT={port} cannot derive an adjacent MCP port"))
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

/// Initialize the global tracing subscriber.
///
/// - `RUST_LOG` controls per-target levels (default `info`).
/// - The fmt layer prints span context inline so every log line from
///   inside a request handler is annotated with the method + URI from
///   tower-http's `TraceLayer` span.
/// - `tracing_log::LogTracer` bridges `log::*!` macros from crates that
///   don't speak tracing (notably `driver`) into the same subscriber.
fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let fmt_layer = tracing_subscriber::fmt::layer().with_target(false);
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(fmt_layer)
        .try_init();
    let _ = tracing_log::LogTracer::init();
}
