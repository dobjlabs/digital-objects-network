use std::{net::SocketAddr, sync::Arc};

use alloy::primitives::Address;
use anyhow::Result;
use axum::{
    extract::State,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use pod2::middleware::Hash;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tracing::info;
use wire_types::synchronizer::{
    GroundingWitnessRequest, GroundingWitnessResponse, MembershipRequest, MembershipResponse,
    NullifierContainsEntry, NullifierContainsRequest, NullifierContainsResponse,
    ObjectContainsEntry, ObjectContainsRequest, ObjectContainsResponse, ObjectProofResponse,
    StateHeadResponse, SyncProgressResponse,
};
use wire_types::HealthResponse;

use crate::{
    key_value_db::KeyValueDb,
    sync_db::{HeadSnapshot, SyncDb},
};

const MAX_HASH_QUERY_ITEMS: usize = 256;

#[derive(Clone)]
struct AppState {
    /// Key-value (RocksDB) read path used for membership checks and Merkle proofs.
    key_value_db: KeyValueDb,
    /// Postgres-backed committed-state and sync-progress store.
    sync_db: Arc<SyncDb>,
    info: Arc<Info>,
}

/// Membership of each queried commitment and nullifier, answered against one head snapshot.
struct Membership {
    head: HeadSnapshot,
    created_present: Vec<bool>,
    nullifier_present: Vec<bool>,
}

pub async fn run_api_server(
    sync_db: Arc<SyncDb>,
    key_value_db: KeyValueDb,
    info: Arc<Info>,
    bind_addr: SocketAddr,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<()> {
    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/info", get(get_info))
        .route("/sync-progress", get(get_sync_progress))
        .route("/v1/state/head", get(get_state_head))
        .route("/v1/state/membership", post(post_state_membership))
        .route(
            "/v1/state/object/contains",
            post(post_state_object_contains),
        )
        .route(
            "/v1/state/nullifier/contains",
            post(post_state_nullifier_contains),
        )
        .route("/v1/txlib/grounding-witness", post(post_grounding_witness))
        .layer(tower_http::cors::CorsLayer::permissive())
        .with_state(AppState {
            key_value_db,
            sync_db,
            info,
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

/// Release tag stamped by build.rs ("dev" outside a release build).
const RELEASE_TAG: &str = env!("DOBJ_RELEASE_TAG");
/// Target triple stamped by build.rs.
const TARGET_TRIPLE: &str = env!("DOBJ_TARGET_TRIPLE");

async fn healthz() -> Json<HealthResponse> {
    Json(HealthResponse::stamped(RELEASE_TAG, TARGET_TRIPLE))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Info {
    pub version: String,
    pub init_start_slot: u32,
    pub to_address: Address,
    pub txn_finalized_pred: String,
    pub verifier_data_hash: String,
}

async fn get_info(State(app_state): State<AppState>) -> Json<Info> {
    let info = (*app_state.info).clone();
    Json(info)
}

async fn get_sync_progress(
    State(app_state): State<AppState>,
) -> Result<Json<SyncProgressResponse>, (StatusCode, String)> {
    let head = load_head_snapshot(&app_state).await?;
    Ok(Json(SyncProgressResponse {
        last_processed_slot: head.last_processed_slot,
        last_processed_block_number: head.last_processed_block_number,
    }))
}

async fn get_state_head(
    State(app_state): State<AppState>,
) -> Result<Json<StateHeadResponse>, (StatusCode, String)> {
    let head = load_head_snapshot(&app_state).await?;
    let metadata = head.state.metadata;
    Ok(Json(StateHeadResponse {
        last_processed_slot: head.last_processed_slot,
        last_processed_block_number: head.last_processed_block_number,
        state_root: metadata.state_root,
        block_number: metadata.block.number as i64,
        created_count: metadata.created_count as usize,
        nullifier_count: metadata.nullifier_count as usize,
        state_root_count: metadata.state_root_count as usize,
    }))
}

async fn post_state_object_contains(
    State(app_state): State<AppState>,
    Json(body): Json<ObjectContainsRequest>,
) -> Result<Json<ObjectContainsResponse>, (StatusCode, String)> {
    let membership = load_membership(&app_state, &body.object_commitments, &[]).await?;
    Ok(Json(ObjectContainsResponse {
        last_processed_slot: membership.head.last_processed_slot,
        state_root: membership.head.state.metadata.state_root,
        results: object_contains_entries(&body.object_commitments, membership.created_present),
    }))
}

async fn post_state_nullifier_contains(
    State(app_state): State<AppState>,
    Json(body): Json<NullifierContainsRequest>,
) -> Result<Json<NullifierContainsResponse>, (StatusCode, String)> {
    let membership = load_membership(&app_state, &[], &body.nullifiers).await?;
    Ok(Json(NullifierContainsResponse {
        last_processed_slot: membership.head.last_processed_slot,
        state_root: membership.head.state.metadata.state_root,
        results: nullifier_contains_entries(&body.nullifiers, membership.nullifier_present),
    }))
}

async fn post_state_membership(
    State(app_state): State<AppState>,
    Json(body): Json<MembershipRequest>,
) -> Result<Json<MembershipResponse>, (StatusCode, String)> {
    let membership =
        load_membership(&app_state, &body.object_commitments, &body.nullifiers).await?;
    Ok(Json(MembershipResponse {
        last_processed_slot: membership.head.last_processed_slot,
        state_root: membership.head.state.metadata.state_root,
        created_results: object_contains_entries(
            &body.object_commitments,
            membership.created_present,
        ),
        nullifier_results: nullifier_contains_entries(
            &body.nullifiers,
            membership.nullifier_present,
        ),
    }))
}

async fn post_grounding_witness(
    State(app_state): State<AppState>,
    Json(body): Json<GroundingWitnessRequest>,
) -> Result<Json<GroundingWitnessResponse>, (StatusCode, String)> {
    ensure_hash_query_limit("objectCommitments", body.object_commitments.len())?;
    let object_commitments = body.object_commitments;
    let (snapshot, indices) = app_state
        .sync_db
        .snapshot_with_created_indices(&object_commitments)
        .await
        .map_err(internal_error)?;
    let state = snapshot.ok_or_else(not_initialized)?.state;
    let witnesses = app_state
        .key_value_db
        .prove_created_for(&state.roots, &object_commitments, &indices)
        .map_err(internal_error)?;
    let state_header = state.state_header();

    Ok(Json(GroundingWitnessResponse {
        state_root: state.metadata.state_root,
        block_number: state_header.block_number,
        block_timestamp: state_header.block_timestamp,
        block_hash: state_header.block_hash,
        created_root: state_header.created_root,
        nullifiers_root: state_header.nullifiers_root,
        prior_state_history_root: state_header.prior_state_history_root,
        created_proofs: object_commitments
            .into_iter()
            .zip(witnesses)
            .map(|(commitment, witness)| {
                let (index, proof) = witness.unzip();
                ObjectProofResponse {
                    commitment,
                    present: index.is_some(),
                    index,
                    proof,
                }
            })
            .collect(),
    }))
}

async fn load_membership(
    app_state: &AppState,
    object_commitments: &[Hash],
    nullifiers: &[Hash],
) -> Result<Membership, (StatusCode, String)> {
    ensure_membership_query_limit(object_commitments.len(), nullifiers.len())?;
    let (snapshot, indices) = app_state
        .sync_db
        .snapshot_with_created_indices(object_commitments)
        .await
        .map_err(internal_error)?;
    let head = snapshot.ok_or_else(not_initialized)?;
    let key_value_db = &app_state.key_value_db;
    Ok(Membership {
        created_present: key_value_db
            .created_exists_for(&head.state.roots, object_commitments, &indices)
            .map_err(internal_error)?,
        nullifier_present: key_value_db
            .nullifier_exists_batch(&head.state.roots, nullifiers)
            .map_err(internal_error)?,
        head,
    })
}

async fn load_head_snapshot(app_state: &AppState) -> Result<HeadSnapshot, (StatusCode, String)> {
    app_state
        .sync_db
        .head_snapshot()
        .await
        .map_err(internal_error)?
        .ok_or_else(not_initialized)
}

fn not_initialized() -> (StatusCode, String) {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "synchronizer has not derived its genesis state yet".to_string(),
    )
}

fn ensure_hash_query_limit(field_name: &str, count: usize) -> Result<(), (StatusCode, String)> {
    if count > MAX_HASH_QUERY_ITEMS {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("{field_name} exceeds maximum item count: {count} > {MAX_HASH_QUERY_ITEMS}"),
        ));
    }
    Ok(())
}

fn ensure_membership_query_limit(
    object_commitment_count: usize,
    nullifier_count: usize,
) -> Result<(), (StatusCode, String)> {
    let total = object_commitment_count.saturating_add(nullifier_count);
    if total > MAX_HASH_QUERY_ITEMS {
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "membership query exceeds maximum total item count: {total} > {MAX_HASH_QUERY_ITEMS} across object commitments and nullifiers"
            ),
        ));
    }
    Ok(())
}

fn object_contains_entries(
    object_commitments: &[Hash],
    created_present: Vec<bool>,
) -> Vec<ObjectContainsEntry> {
    object_commitments
        .iter()
        .zip(created_present)
        .map(|(&commitment, present)| ObjectContainsEntry {
            commitment,
            present,
        })
        .collect()
}

fn nullifier_contains_entries(
    nullifiers: &[Hash],
    nullifier_present: Vec<bool>,
) -> Vec<NullifierContainsEntry> {
    nullifiers
        .iter()
        .zip(nullifier_present)
        .map(|(&nullifier, present)| NullifierContainsEntry { nullifier, present })
        .collect()
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
