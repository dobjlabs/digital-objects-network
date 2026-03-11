use std::{net::SocketAddr, sync::Arc};

use anyhow::Result;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use hex::{FromHex, ToHex};
use pod2::middleware::Hash;
use synchronizer::api_types::{
    StateFullResponse, StateHeadResponse, SyncProgressResponse, TxContainsEntry, TxContainsRequest,
    TxContainsResponse, TxStatusResponse,
};
use tokio::sync::watch;
use tracing::info;

use crate::{state_machine::StateMachine, sync_db::SyncDb};

#[derive(Clone)]
struct AppState {
    sync_db: Arc<SyncDb>,
    state_machine: Arc<StateMachine>,
}

struct HeadSnapshot {
    last_processed_slot: Option<u32>,
    last_processed_block_number: Option<u32>,
    current_gsr: Option<String>,
    current_block_number: Option<i64>,
    tx_count: usize,
    nullifier_count: usize,
    gsr_count: usize,
}

pub async fn run_api_server(
    sync_db: Arc<SyncDb>,
    state_machine: Arc<StateMachine>,
    bind_addr: SocketAddr,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<()> {
    let app = Router::new()
        .route("/sync-progress", get(get_sync_progress))
        .route("/v1/state/head", get(get_state_head))
        .route("/v1/state/full", get(get_state_full))
        .route("/v1/state/tx/contains", post(post_state_tx_contains))
        .route("/v1/state/tx/{tx_hash}", get(get_state_tx))
        .with_state(AppState {
            sync_db,
            state_machine,
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

async fn get_state_head(
    State(app_state): State<AppState>,
) -> Result<Json<StateHeadResponse>, (StatusCode, String)> {
    let head = build_head_snapshot(&app_state).await?;
    Ok(Json(StateHeadResponse {
        last_processed_slot: head.last_processed_slot,
        last_processed_block_number: head.last_processed_block_number,
        current_gsr: head.current_gsr,
        current_block_number: head.current_block_number,
        tx_count: head.tx_count,
        nullifier_count: head.nullifier_count,
        gsr_count: head.gsr_count,
    }))
}

async fn get_state_full(
    State(app_state): State<AppState>,
) -> Result<Json<StateFullResponse>, (StatusCode, String)> {
    let snapshot = app_state
        .state_machine
        .api_state_snapshot()
        .map_err(internal_error)?;

    let mut transactions = snapshot
        .transactions
        .iter()
        .map(encode_hash_hex)
        .collect::<Vec<_>>();
    transactions.sort();

    let mut nullifiers = snapshot
        .nullifiers
        .iter()
        .map(encode_hash_hex)
        .collect::<Vec<_>>();
    nullifiers.sort();

    let prior_gsrs = if snapshot.global_state_roots.is_empty() {
        Vec::new()
    } else {
        snapshot.global_state_roots[..snapshot.global_state_roots.len() - 1].to_vec()
    };
    let gsrs = prior_gsrs.iter().map(encode_hash_hex).collect::<Vec<_>>();

    Ok(Json(StateFullResponse {
        block_number: snapshot.current_block_number.unwrap_or(0),
        current_gsr: snapshot.current_gsr.as_ref().map(encode_hash_hex),
        transactions,
        nullifiers,
        gsrs,
    }))
}

async fn post_state_tx_contains(
    State(app_state): State<AppState>,
    Json(body): Json<TxContainsRequest>,
) -> Result<Json<TxContainsResponse>, (StatusCode, String)> {
    let hashes = body
        .tx_hashes
        .iter()
        .map(|raw| parse_hash_hex(raw))
        .collect::<Result<Vec<_>, _>>()?;
    let present = app_state
        .state_machine
        .tx_exists_batch(&hashes)
        .map_err(internal_error)?;
    let head = build_head_snapshot(&app_state).await?;

    let results = hashes
        .iter()
        .zip(present.into_iter())
        .map(|(hash, present)| TxContainsEntry {
            tx_hash: encode_hash_hex(hash),
            present,
        })
        .collect();

    Ok(Json(TxContainsResponse {
        last_processed_slot: head.last_processed_slot,
        current_gsr: head.current_gsr,
        results,
    }))
}

async fn get_state_tx(
    State(app_state): State<AppState>,
    Path(tx_hash): Path<String>,
) -> Result<Json<TxStatusResponse>, (StatusCode, String)> {
    let hash = parse_hash_hex(&tx_hash)?;
    let present = app_state
        .state_machine
        .tx_exists(&hash)
        .map_err(internal_error)?;
    let head = build_head_snapshot(&app_state).await?;
    Ok(Json(TxStatusResponse {
        tx_hash: encode_hash_hex(&hash),
        present,
        last_processed_slot: head.last_processed_slot,
        current_gsr: head.current_gsr,
    }))
}

async fn build_head_snapshot(app_state: &AppState) -> Result<HeadSnapshot, (StatusCode, String)> {
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

    let snapshot = app_state
        .state_machine
        .api_state_snapshot()
        .map_err(internal_error)?;

    Ok(HeadSnapshot {
        last_processed_slot,
        last_processed_block_number,
        current_gsr: snapshot.current_gsr.as_ref().map(encode_hash_hex),
        current_block_number: snapshot.current_block_number,
        tx_count: snapshot.transactions.len(),
        nullifier_count: snapshot.nullifiers.len(),
        gsr_count: snapshot.global_state_roots.len(),
    })
}

fn parse_hash_hex(value: &str) -> Result<Hash, (StatusCode, String)> {
    let trimmed = value.trim().strip_prefix("0x").unwrap_or(value.trim());
    Hash::from_hex(trimmed).map_err(|err| {
        (
            StatusCode::BAD_REQUEST,
            format!("invalid hash `{value}`: {err}"),
        )
    })
}

fn encode_hash_hex(hash: &Hash) -> String {
    format!("0x{}", hash.encode_hex::<String>())
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
