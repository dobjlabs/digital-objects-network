use std::{net::SocketAddr, sync::Arc};

use anyhow::Result;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use hex::FromHex;
use pod2::middleware::Hash;
use synchronizer::api_types::{
    GroundingWitnessRequest, GroundingWitnessResponse, HealthResponse, MembershipRequest,
    MembershipResponse, NullifierContainsEntry, NullifierContainsRequest,
    NullifierContainsResponse, SourceTxProofResponse, StateHeadResponse, SyncProgressResponse,
    TxContainsEntry, TxContainsRequest, TxContainsResponse, TxStatusResponse,
};
use tokio::sync::watch;
use tracing::info;

use crate::{app_db::AppHead, state_machine::StateMachine, sync_db::SyncDb};
use common::encode_hash_hex;

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
        .route("/healthz", get(healthz))
        .route("/sync-progress", get(get_sync_progress))
        .route("/v1/state/head", get(get_state_head))
        .route("/v1/state/membership", post(post_state_membership))
        .route("/v1/state/tx/contains", post(post_state_tx_contains))
        .route(
            "/v1/state/nullifier/contains",
            post(post_state_nullifier_contains),
        )
        .route("/v1/state/tx/{tx_hash}", get(get_state_tx))
        .route("/v1/txlib/grounding-witness", post(post_grounding_witness))
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

async fn healthz() -> Json<HealthResponse> {
    Json(HealthResponse { ok: true })
}

async fn get_sync_progress(
    State(app_state): State<AppState>,
) -> Result<Json<SyncProgressResponse>, (StatusCode, String)> {
    let (last_processed_slot, last_processed_block_number) = load_sync_progress(&app_state).await?;

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

async fn post_state_tx_contains(
    State(app_state): State<AppState>,
    Json(body): Json<TxContainsRequest>,
) -> Result<Json<TxContainsResponse>, (StatusCode, String)> {
    let hashes = body
        .tx_hashes
        .iter()
        .map(|raw| parse_hash_hex(raw))
        .collect::<Result<Vec<_>, _>>()?;
    let membership = app_state
        .state_machine
        .membership_snapshot(&hashes, &[])
        .map_err(internal_error)?;
    let head = build_head_snapshot_from_head(&app_state, membership.head).await?;

    let results = hashes
        .iter()
        .zip(membership.tx_present.into_iter())
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

async fn post_state_nullifier_contains(
    State(app_state): State<AppState>,
    Json(body): Json<NullifierContainsRequest>,
) -> Result<Json<NullifierContainsResponse>, (StatusCode, String)> {
    let hashes = body
        .nullifiers
        .iter()
        .map(|raw| parse_hash_hex(raw))
        .collect::<Result<Vec<_>, _>>()?;
    let membership = app_state
        .state_machine
        .membership_snapshot(&[], &hashes)
        .map_err(internal_error)?;
    let head = build_head_snapshot_from_head(&app_state, membership.head).await?;

    let results = hashes
        .iter()
        .zip(membership.nullifier_present.into_iter())
        .map(|(hash, present)| NullifierContainsEntry {
            nullifier: encode_hash_hex(hash),
            present,
        })
        .collect();

    Ok(Json(NullifierContainsResponse {
        last_processed_slot: head.last_processed_slot,
        current_gsr: head.current_gsr,
        results,
    }))
}

async fn post_state_membership(
    State(app_state): State<AppState>,
    Json(body): Json<MembershipRequest>,
) -> Result<Json<MembershipResponse>, (StatusCode, String)> {
    let tx_hashes = body
        .tx_hashes
        .iter()
        .map(|raw| parse_hash_hex(raw))
        .collect::<Result<Vec<_>, _>>()?;
    let nullifiers = body
        .nullifiers
        .iter()
        .map(|raw| parse_hash_hex(raw))
        .collect::<Result<Vec<_>, _>>()?;
    let membership = app_state
        .state_machine
        .membership_snapshot(&tx_hashes, &nullifiers)
        .map_err(internal_error)?;
    let head = build_head_snapshot_from_head(&app_state, membership.head).await?;

    let tx_results = tx_hashes
        .iter()
        .zip(membership.tx_present.into_iter())
        .map(|(hash, present)| TxContainsEntry {
            tx_hash: encode_hash_hex(hash),
            present,
        })
        .collect();
    let nullifier_results = nullifiers
        .iter()
        .zip(membership.nullifier_present.into_iter())
        .map(|(hash, present)| NullifierContainsEntry {
            nullifier: encode_hash_hex(hash),
            present,
        })
        .collect();

    Ok(Json(MembershipResponse {
        last_processed_slot: head.last_processed_slot,
        current_gsr: head.current_gsr,
        tx_results,
        nullifier_results,
    }))
}

async fn get_state_tx(
    State(app_state): State<AppState>,
    Path(tx_hash): Path<String>,
) -> Result<Json<TxStatusResponse>, (StatusCode, String)> {
    let hash = parse_hash_hex(&tx_hash)?;
    let membership = app_state
        .state_machine
        .membership_snapshot(std::slice::from_ref(&hash), &[])
        .map_err(internal_error)?;
    let head = build_head_snapshot_from_head(&app_state, membership.head).await?;
    Ok(Json(TxStatusResponse {
        tx_hash: encode_hash_hex(&hash),
        present: membership.tx_present[0],
        last_processed_slot: head.last_processed_slot,
        current_gsr: head.current_gsr,
    }))
}

async fn post_grounding_witness(
    State(app_state): State<AppState>,
    Json(body): Json<GroundingWitnessRequest>,
) -> Result<Json<GroundingWitnessResponse>, (StatusCode, String)> {
    let source_tx_hashes = body
        .source_tx_hashes
        .iter()
        .map(|raw| parse_hash_hex(raw))
        .collect::<Result<Vec<_>, _>>()?;
    let snapshot = app_state
        .state_machine
        .grounding_witness(&source_tx_hashes)
        .map_err(internal_error)?;
    let head = snapshot.head;
    let state_root = head.current_state_root().ok_or_else(|| {
        (
            StatusCode::CONFLICT,
            "synchronizer has no canonical grounded state yet".to_string(),
        )
    })?;
    let state_root_hash = head.current_gsr.unwrap_or_else(|| state_root.hash());

    Ok(Json(GroundingWitnessResponse {
        state_root_hash: encode_hash_hex(&state_root_hash),
        block_number: state_root.block_number,
        transactions_root: encode_hash_hex(&state_root.transactions_root),
        nullifiers_root: encode_hash_hex(&state_root.nullifiers_root),
        gsrs_root: encode_hash_hex(&state_root.gsrs_root),
        source_tx_proofs: snapshot
            .source_tx_proofs
            .into_iter()
            .map(|entry| SourceTxProofResponse {
                tx_hash: encode_hash_hex(&entry.tx_hash),
                present: entry.present,
                proof: entry.proof,
            })
            .collect(),
    }))
}

async fn build_head_snapshot(app_state: &AppState) -> Result<HeadSnapshot, (StatusCode, String)> {
    let head = app_state
        .state_machine
        .head_snapshot()
        .map_err(internal_error)?;
    build_head_snapshot_from_head(app_state, head).await
}

async fn build_head_snapshot_from_head(
    app_state: &AppState,
    head: AppHead,
) -> Result<HeadSnapshot, (StatusCode, String)> {
    let (last_processed_slot, last_processed_block_number) = load_sync_progress(app_state).await?;

    Ok(HeadSnapshot {
        last_processed_slot,
        last_processed_block_number,
        current_gsr: head.current_gsr.as_ref().map(encode_hash_hex),
        current_block_number: head.current_block_number.map(i64::from),
        tx_count: head.tx_count as usize,
        nullifier_count: head.nullifier_count as usize,
        gsr_count: head.gsr_count as usize,
    })
}

async fn load_sync_progress(
    app_state: &AppState,
) -> Result<(Option<u32>, Option<u32>), (StatusCode, String)> {
    let progress = app_state
        .sync_db
        .last_progress()
        .await
        .map_err(internal_error)?;
    Ok(match progress {
        Some(progress) => (
            Some(progress.last_processed_slot),
            progress.last_processed_block_number,
        ),
        None => (None, None),
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
