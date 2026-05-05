use std::sync::Arc;

use anyhow::Result;
use driver::Driver;

mod error;
mod events;
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

    let state = AppState::new(driver, event_tx);
    if let Some(ref dir) = static_dir {
        eprintln!("dobjd: serving static frontend from {}", dir.display());
    }
    let app = routes::router(state, static_dir);

    let addr = format!("127.0.0.1:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    eprintln!("dobjd: listening on http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}
