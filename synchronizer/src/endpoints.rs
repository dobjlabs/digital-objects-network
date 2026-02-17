use std::{net::SocketAddr, sync::Arc};

use anyhow::Result;
use axum::{extract::State, http::StatusCode, routing::get, Json, Router};
use serde::Serialize;
use tokio::sync::watch;
use tracing::info;

use crate::node::Node;

#[derive(Clone)]
struct AppState {
    node: Arc<Node>,
}

#[derive(Serialize)]
struct StateResponse {
    transactions: Vec<String>,
    nullifiers: Vec<String>,
    last_processed_slot: Option<u32>,
    last_processed_block_number: Option<u32>,
}

pub async fn run_server(
    node: Arc<Node>,
    bind_addr: SocketAddr,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<()> {
    let app = Router::new()
        .route("/state", get(get_state))
        .with_state(AppState { node });

    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    info!(%bind_addr, "State API listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            loop {
                if *shutdown_rx.borrow() {
                    break;
                }
                if shutdown_rx.changed().await.is_err() {
                    break;
                }
            }
        })
        .await?;
    Ok(())
}

async fn get_state(
    State(app_state): State<AppState>,
) -> Result<Json<StateResponse>, (StatusCode, String)> {
    let (mut transactions, mut nullifiers) =
        app_state.node.state_snapshot().map_err(internal_error)?;
    transactions.sort_unstable();
    nullifiers.sort_unstable();

    let progress = app_state
        .node
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

    Ok(Json(StateResponse {
        transactions,
        nullifiers,
        last_processed_slot,
        last_processed_block_number,
    }))
}

fn internal_error(err: anyhow::Error) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}
