use std::{net::SocketAddr, sync::Arc};

use anyhow::Result;
use axum::{extract::State, http::StatusCode, routing::get, Json, Router};
use serde::Serialize;
use tokio::sync::watch;
use tracing::info;

use crate::app_db::hash_to_hex;
use crate::state_machine::StateMachine;
use crate::sync_db::SyncDb;

#[derive(Clone)]
struct AppState {
    state_machine: Arc<StateMachine>,
    sync_db: Arc<SyncDb>,
}

#[derive(Serialize)]
struct StateResponse {
    transactions: Vec<String>,
    nullifiers: Vec<String>,
    current_gsr: Option<String>,
    last_processed_slot: Option<u32>,
    last_processed_block_number: Option<u32>,
}

pub async fn run_api_server(
    state_machine: Arc<StateMachine>,
    sync_db: Arc<SyncDb>,
    bind_addr: SocketAddr,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<()> {
    let app = Router::new()
        .route("/state", get(get_state))
        .with_state(AppState {
            state_machine,
            sync_db,
        });

    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    info!(%bind_addr, "API server listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            wait_for_shutdown(&mut shutdown_rx).await;
        })
        .await?;
    Ok(())
}

async fn get_state(
    State(app_state): State<AppState>,
) -> Result<Json<StateResponse>, (StatusCode, String)> {
    let (transactions, nullifiers, global_state_roots) = app_state
        .state_machine
        .state_snapshot()
        .map_err(internal_error)?;

    let mut transactions: Vec<String> = transactions.iter().map(hash_to_hex).collect();
    let mut nullifiers: Vec<String> = nullifiers.iter().map(hash_to_hex).collect();
    transactions.sort_unstable();
    nullifiers.sort_unstable();

    let current_gsr = global_state_roots.last().map(hash_to_hex);

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

    Ok(Json(StateResponse {
        transactions,
        nullifiers,
        current_gsr,
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
