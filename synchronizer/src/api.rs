use std::{net::SocketAddr, sync::Arc};

use anyhow::Result;
use axum::{extract::State, http::StatusCode, routing::get, Json, Router};
use serde::Serialize;
use tokio::sync::watch;
use tracing::info;

use crate::sync_db::SyncDb;

#[derive(Clone)]
struct AppState {
    sync_db: Arc<SyncDb>,
}

#[derive(Serialize)]
struct SyncProgressResponse {
    last_processed_slot: Option<u32>,
    last_processed_block_number: Option<u32>,
}

pub async fn run_api_server(
    sync_db: Arc<SyncDb>,
    bind_addr: SocketAddr,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<()> {
    let app = Router::new()
        .route("/sync-progress", get(get_sync_progress))
        .with_state(AppState { sync_db });

    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    info!(%bind_addr, "API server listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            wait_for_shutdown(&mut shutdown_rx).await;
        })
        .await?;
    Ok(())
}

async fn get_sync_progress(
    State(app_state): State<AppState>,
) -> Result<Json<SyncProgressResponse>, (StatusCode, String)> {
    let progress = app_state
        .sync_db
        .last_progress()
        .await
        .map_err(internal_error)?;
    let (last_processed_slot, last_processed_block_number) = match progress {
        Some(progress) => (
            Some(progress.last_processed_slot),
            progress.last_processed_block_number,
        ),
        None => (None, None),
    };

    Ok(Json(SyncProgressResponse {
        last_processed_slot,
        last_processed_block_number,
    }))
}

fn internal_error(err: anyhow::Error) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}

async fn wait_for_shutdown(shutdown_rx: &mut watch::Receiver<bool>) {
    loop {
        if *shutdown_rx.borrow() {
            break;
        }
        if shutdown_rx.changed().await.is_err() {
            break;
        }
    }
}
