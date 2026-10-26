use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use driver::Driver;
use tracing_subscriber::{EnvFilter, prelude::*};

mod error;
mod events;
mod jobs;
mod mcp;
mod parent_lifetime;
mod registry;
mod routes;
mod runs;
mod settle;
mod state;
mod ui;
mod web_access;

use jobs::JobRegistry;
use runs::RunRegistry;
use settle::Settler;
use state::{AppState, Services};

const DEFAULT_HTTP_PORT: u16 = 7717;

/// Release tag stamped by build.rs ("dev" outside a release build).
pub(crate) const RELEASE_TAG: &str = env!("DOBJ_RELEASE_TAG");
/// Target triple stamped by build.rs.
pub(crate) const TARGET_TRIPLE: &str = env!("DOBJ_TARGET_TRIPLE");

fn main() -> Result<()> {
    // Print the stamp and exit without touching ports or the driver state.
    // `dobj update` runs this to validate a freshly installed binary.
    if std::env::args().any(|arg| arg == "--version") {
        println!("dobjd {RELEASE_TAG} ({TARGET_TRIPLE})");
        return Ok(());
    }

    parent_lifetime::install_if_requested()?;
    run()
}

#[tokio::main]
async fn run() -> Result<()> {
    init_tracing();
    tracing::info!("dobjd {RELEASE_TAG} ({TARGET_TRIPLE})");
    // A supervisor that sets DOBJ_INSTANCE_ID supplies the environment; .env must not override it.
    if std::env::var_os("DOBJ_INSTANCE_ID").is_none()
        && let Err(err) = payload::load_dotenv()
    {
        tracing::warn!("failed to load env: {err}");
    }

    let port = http_port_from_env()?;
    let mcp_port = mcp_port_for_http_port(port)?;

    let driver = Arc::new(Driver::open_default()?);
    let web_access = web_access::WebAccess::new(port);
    let events = events::EventHub::new();
    let runs = RunRegistry::new(max_concurrent_proofs_from_env()?);
    let settler = Arc::new(Settler::new(driver.clone(), events.clone()));
    let services = Services {
        driver: driver.clone(),
        events,
        runs: runs.clone(),
        jobs: JobRegistry::default(),
        settler: settler.clone(),
    };

    let mcp_runtime = Arc::new(mcp::McpRuntime::new(
        services.clone(),
        format!("127.0.0.1:{mcp_port}"),
        port,
    ));

    let state = AppState::new(services.clone(), mcp_runtime.clone());
    let app = routes::router(state, web_access);

    // Bind both ports up-front (HTTP here, MCP via `prebind`) so startup
    // fails fast and synchronously if a port is taken -- a half-running
    // dobjd is worse than no dobjd. The MCP listener is only served once
    // the circuits are warm (the `apply` below), so the first MCP action
    // does not pay the cold-build cost.
    let addr = format!("127.0.0.1:{port}");
    let http_listener = tokio::net::TcpListener::bind(&addr).await?;

    let mcp_enabled = driver.load_settings()?.mcp_enabled;
    mcp_runtime.prebind(mcp_enabled).await?;
    if !mcp_enabled {
        tracing::info!("MCP server disabled by settings (mcpEnabled=false)");
    }

    // Load the proving circuits (recursive MainPod, empty pod, and the VDF +
    // lt_eq_u256 intro pods) before accepting requests, so the first action
    // doesn't pay to build them. Ports are already bound, so a port conflict
    // still fails fast ahead of this. This only touches circuit data (no
    // proving), so on a warm cache it's fast reads. Runs on the blocking pool
    // since circuit construction is CPU-bound and synchronous. A failure is
    // fatal (the warm panics internally): a circuit that can't build now would
    // fail every action, so refuse to start rather than serve a daemon that
    // cannot prove.
    tokio::task::spawn_blocking(driver::warm_proving_circuits)
        .await
        .map_err(|err| anyhow!("circuit warm-up task panicked: {err}"))?;

    // Circuits are warm; serve the pre-bound MCP listener (no-op when
    // disabled). Deferred to here so an MCP action can't land mid-warm-up.
    mcp_runtime.apply(mcp_enabled).await?;

    let reaper = services.clone();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(registry::REAP_INTERVAL);
        loop {
            ticker.tick().await;
            reaper.runs.entries.reap(registry::RETENTION);
            reaper.jobs.reap(registry::RETENTION);
        }
    });

    // Resume settlement for persisted transactions after a restart.
    tokio::spawn(settler.run_loop());

    tracing::info!("listening on http://{addr}");
    tracing::info!("browser UI: http://{addr}{}", ui::home_path());
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

/// Read the run proof limit from `DOBJD_MAX_PROOFS`; default to one.
fn max_concurrent_proofs_from_env() -> Result<usize> {
    match std::env::var("DOBJD_MAX_PROOFS") {
        Ok(value) => value
            .parse::<usize>()
            .ok()
            .filter(|count| *count >= 1)
            .with_context(|| format!("invalid DOBJD_MAX_PROOFS={value:?}, expected a count >= 1")),
        Err(std::env::VarError::NotPresent) => Ok(runs::DEFAULT_MAX_CONCURRENT_PROOFS),
        Err(err) => Err(anyhow!("invalid DOBJD_MAX_PROOFS env: {err}")),
    }
}

fn mcp_port_for_http_port(port: u16) -> Result<u16> {
    port.checked_add(1)
        .ok_or_else(|| anyhow!("DOBJD_PORT={port} cannot derive an adjacent MCP port"))
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
