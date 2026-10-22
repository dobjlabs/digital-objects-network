use std::{collections::HashMap, net::SocketAddr, sync::Arc};

use alloy::primitives::Address;
use anyhow::Result;
use axum::{
    extract::State,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use pod2::{backends::plonky2::primitives::merkletree::MerkleProof, middleware::Hash};
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
    don_state::StateRoots,
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

/// Internal view of head/progress fields shaped for HTTP responses.
struct HeadView {
    /// Last slot fully committed by the synchronizer.
    last_processed_slot: u32,
    /// Execution block number associated with the last processed slot, if any.
    last_processed_block_number: Option<u32>,
    /// Head state root, if one exists.
    current_state_root: Option<Hash>,
    /// Execution block number committed inside the head state root, if any.
    current_block_number: Option<i64>,
    /// Number of objects in the global created set.
    created_count: usize,
    /// Number of spent nullifiers in committed state.
    nullifier_count: usize,
    /// Number of state root entries in chain history.
    state_root_count: usize,
}

#[derive(Debug, Clone)]
/// Membership result anchored to one caller-provided root set.
struct MembershipSnapshot {
    /// Per-request created-object membership bits under `roots.created`.
    created_present: Vec<bool>,
    /// Per-request nullifier membership bits under `roots.nullifiers`.
    nullifier_present: Vec<bool>,
}

#[derive(Debug, Clone)]
/// Membership proof for an object against the head state's created-set root.
struct ObjectMembershipProof {
    /// Object commitment the client asked about.
    commitment: Hash,
    /// Whether the object is present in the committed created set.
    present: bool,
    /// `(array index, ArrayContains proof)` when present, else `None`.
    witness: Option<(i64, MerkleProof)>,
}

#[derive(Debug, Clone)]
/// Proof-bearing result used by txlib to ground action execution.
struct GroundingWitnessSnapshot {
    /// Per-input-object created-set membership proofs under the provided roots.
    object_proofs: Vec<ObjectMembershipProof>,
}

struct MembershipContext {
    head: HeadView,
    object_commitments: Vec<Hash>,
    nullifiers: Vec<Hash>,
    membership: MembershipSnapshot,
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
    let (last_processed_slot, last_processed_block_number) = load_sync_progress(&app_state).await?;

    Ok(Json(SyncProgressResponse {
        last_processed_slot,
        last_processed_block_number,
    }))
}

async fn get_state_head(
    State(app_state): State<AppState>,
) -> Result<Json<StateHeadResponse>, (StatusCode, String)> {
    let snapshot = load_head_snapshot(&app_state).await?;
    let head = build_head_view(&snapshot);
    Ok(Json(StateHeadResponse {
        last_processed_slot: head.last_processed_slot,
        last_processed_block_number: head.last_processed_block_number,
        current_state_root: head.current_state_root,
        current_block_number: head.current_block_number,
        created_count: head.created_count,
        nullifier_count: head.nullifier_count,
        state_root_count: head.state_root_count,
    }))
}

async fn post_state_object_contains(
    State(app_state): State<AppState>,
    Json(body): Json<ObjectContainsRequest>,
) -> Result<Json<ObjectContainsResponse>, (StatusCode, String)> {
    let no_nullifiers: &[Hash] = &[];
    let MembershipContext {
        head,
        object_commitments,
        membership,
        ..
    } = load_membership_context(&app_state, &body.object_commitments, no_nullifiers).await?;

    Ok(Json(ObjectContainsResponse {
        last_processed_slot: head.last_processed_slot,
        current_state_root: head.current_state_root,
        results: object_contains_entries(object_commitments, membership.created_present),
    }))
}

async fn post_state_nullifier_contains(
    State(app_state): State<AppState>,
    Json(body): Json<NullifierContainsRequest>,
) -> Result<Json<NullifierContainsResponse>, (StatusCode, String)> {
    let no_tx_hashes: &[Hash] = &[];
    let MembershipContext {
        head,
        nullifiers,
        membership,
        ..
    } = load_membership_context(&app_state, no_tx_hashes, &body.nullifiers).await?;

    Ok(Json(NullifierContainsResponse {
        last_processed_slot: head.last_processed_slot,
        current_state_root: head.current_state_root,
        results: nullifier_contains_entries(nullifiers, membership.nullifier_present),
    }))
}

async fn post_state_membership(
    State(app_state): State<AppState>,
    Json(body): Json<MembershipRequest>,
) -> Result<Json<MembershipResponse>, (StatusCode, String)> {
    let MembershipContext {
        head,
        object_commitments,
        nullifiers,
        membership,
    } = load_membership_context(&app_state, &body.object_commitments, &body.nullifiers).await?;

    Ok(Json(MembershipResponse {
        last_processed_slot: head.last_processed_slot,
        current_state_root: head.current_state_root,
        created_results: object_contains_entries(object_commitments, membership.created_present),
        nullifier_results: nullifier_contains_entries(nullifiers, membership.nullifier_present),
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
    let snapshot = snapshot.ok_or_else(not_initialized)?;
    let witness = grounding_witness(
        &app_state.key_value_db,
        &snapshot.state.roots,
        &object_commitments,
        &indices,
    )
    .map_err(internal_error)?;
    let state = snapshot.state;
    let state_header = state.state_header();
    let state_root = state.metadata.state_root;

    Ok(Json(GroundingWitnessResponse {
        state_root,
        block_number: state_header.block_number,
        block_timestamp: state_header.block_timestamp,
        block_hash: state_header.block_hash,
        created_root: state_header.created_root,
        nullifiers_root: state_header.nullifiers_root,
        prior_state_history_root: state_header.prior_state_history_root,
        created_proofs: witness
            .object_proofs
            .into_iter()
            .map(|entry| {
                let (index, proof) = match entry.witness {
                    Some((index, proof)) => (Some(index), Some(proof)),
                    None => (None, None),
                };
                ObjectProofResponse {
                    commitment: entry.commitment,
                    present: entry.present,
                    index,
                    proof,
                }
            })
            .collect(),
    }))
}

fn build_head_view(snapshot: &HeadSnapshot) -> HeadView {
    HeadView {
        last_processed_slot: snapshot.last_processed_slot,
        last_processed_block_number: snapshot.last_processed_block_number,
        current_state_root: Some(snapshot.state.metadata.state_root),
        current_block_number: Some(snapshot.state.metadata.block.number as i64),
        created_count: snapshot.state.metadata.created_count as usize,
        nullifier_count: snapshot.state.metadata.nullifier_count as usize,
        state_root_count: snapshot.state.metadata.state_root_count as usize,
    }
}

fn membership_snapshot(
    key_value_db: &KeyValueDb,
    roots: &StateRoots,
    object_commitments: &[Hash],
    nullifiers: &[Hash],
    indices: &HashMap<Hash, i64>,
) -> anyhow::Result<MembershipSnapshot> {
    Ok(MembershipSnapshot {
        created_present: key_value_db.created_exists_for(roots, object_commitments, indices)?,
        nullifier_present: key_value_db.nullifier_exists_batch(roots, nullifiers)?,
    })
}

fn grounding_witness(
    key_value_db: &KeyValueDb,
    roots: &StateRoots,
    object_commitments: &[Hash],
    indices: &HashMap<Hash, i64>,
) -> anyhow::Result<GroundingWitnessSnapshot> {
    let witnesses = key_value_db.prove_created_for(roots, object_commitments, indices)?;
    let object_proofs = object_commitments
        .iter()
        .zip(witnesses)
        .map(|(commitment, witness)| ObjectMembershipProof {
            commitment: *commitment,
            present: witness.is_some(),
            witness,
        })
        .collect();
    Ok(GroundingWitnessSnapshot { object_proofs })
}

async fn load_membership_context(
    app_state: &AppState,
    object_commitments: &[Hash],
    nullifiers: &[Hash],
) -> Result<MembershipContext, (StatusCode, String)> {
    ensure_membership_query_limit(object_commitments.len(), nullifiers.len())?;
    let object_commitments = object_commitments.to_vec();
    let nullifiers = nullifiers.to_vec();
    let (snapshot, indices) = app_state
        .sync_db
        .snapshot_with_created_indices(&object_commitments)
        .await
        .map_err(internal_error)?;
    let snapshot = snapshot.ok_or_else(not_initialized)?;
    let membership = membership_snapshot(
        &app_state.key_value_db,
        &snapshot.state.roots,
        &object_commitments,
        &nullifiers,
        &indices,
    )
    .map_err(internal_error)?;

    Ok(MembershipContext {
        head: build_head_view(&snapshot),
        object_commitments,
        nullifiers,
        membership,
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

/// The synchronizer has not committed its first slot, so it has no state to answer from yet.
fn not_initialized() -> (StatusCode, String) {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "synchronizer has not derived its genesis state yet".to_string(),
    )
}

async fn load_sync_progress(
    app_state: &AppState,
) -> Result<(u32, Option<u32>), (StatusCode, String)> {
    let snapshot = load_head_snapshot(app_state).await?;
    Ok((
        snapshot.last_processed_slot,
        snapshot.last_processed_block_number,
    ))
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
    tx_hash_count: usize,
    nullifier_count: usize,
) -> Result<(), (StatusCode, String)> {
    let total = tx_hash_count.saturating_add(nullifier_count);
    if total > MAX_HASH_QUERY_ITEMS {
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "membership query exceeds maximum total item count: {total} > {MAX_HASH_QUERY_ITEMS} across tx_hashes and nullifiers"
            ),
        ));
    }
    Ok(())
}

fn object_contains_entries(
    object_commitments: Vec<Hash>,
    created_present: Vec<bool>,
) -> Vec<ObjectContainsEntry> {
    object_commitments
        .into_iter()
        .zip(created_present)
        .map(|(hash, present)| ObjectContainsEntry {
            commitment: hash,
            present,
        })
        .collect()
}

fn nullifier_contains_entries(
    nullifiers: Vec<Hash>,
    nullifier_present: Vec<bool>,
) -> Vec<NullifierContainsEntry> {
    nullifiers
        .into_iter()
        .zip(nullifier_present)
        .map(|(hash, present)| NullifierContainsEntry {
            nullifier: hash,
            present,
        })
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
