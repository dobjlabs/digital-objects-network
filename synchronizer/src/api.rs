use std::{collections::HashMap, net::SocketAddr, sync::Arc};

use anyhow::Result;
use axum::{
    extract::State,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use pod2::{backends::plonky2::primitives::merkletree::MerkleProof, middleware::Hash};
use tokio::sync::watch;
use tracing::info;
use wire_types::synchronizer::{
    GroundingWitnessRequest, GroundingWitnessResponse, HealthResponse, MembershipRequest,
    MembershipResponse, NullifierContainsEntry, NullifierContainsRequest,
    NullifierContainsResponse, ObjectContainsEntry, ObjectContainsRequest, ObjectContainsResponse,
    ObjectProofResponse, StateHeadResponse, SyncProgressResponse,
};

use crate::{
    app_db::AppDb,
    head::CanonicalRoots,
    sync_db::{CurrentSnapshot, SyncDb},
};

const MAX_HASH_QUERY_ITEMS: usize = 256;

#[derive(Clone)]
struct AppState {
    /// RocksDB-backed read path used for membership checks and Merkle proofs.
    app_db: AppDb,
    /// Postgres-backed canonical head and sync-progress store.
    sync_db: Arc<SyncDb>,
}

/// Internal view of head/progress fields shaped for HTTP responses.
struct HeadSnapshot {
    /// Last canonical slot fully committed by the synchronizer.
    last_processed_slot: u32,
    /// Execution block number associated with the last processed slot, if any.
    last_processed_block_number: Option<u32>,
    /// Current canonical global state root, if one exists.
    current_gsr: Option<Hash>,
    /// Execution block number committed inside the current state root, if any.
    current_block_number: Option<i64>,
    /// Number of objects in the canonical global created set.
    created_count: usize,
    /// Number of spent nullifiers in canonical state.
    nullifier_count: usize,
    /// Number of GSR entries in canonical history.
    gsr_count: usize,
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
/// Membership proof for an object against the current created-set root.
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
    head: HeadSnapshot,
    object_commitments: Vec<Hash>,
    nullifiers: Vec<Hash>,
    membership: MembershipSnapshot,
}

pub async fn run_api_server(
    sync_db: Arc<SyncDb>,
    app_db: AppDb,
    bind_addr: SocketAddr,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<()> {
    let app = Router::new()
        .route("/healthz", get(healthz))
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
        .with_state(AppState { app_db, sync_db });

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
    let snapshot = load_current_snapshot(&app_state).await?;
    let head = build_head_snapshot(&snapshot);
    Ok(Json(StateHeadResponse {
        last_processed_slot: head.last_processed_slot,
        last_processed_block_number: head.last_processed_block_number,
        current_gsr: head.current_gsr,
        current_block_number: head.current_block_number,
        created_count: head.created_count,
        nullifier_count: head.nullifier_count,
        gsr_count: head.gsr_count,
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
        current_gsr: head.current_gsr,
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
        current_gsr: head.current_gsr,
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
        current_gsr: head.current_gsr,
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
    let witness = grounding_witness(
        &app_state.app_db,
        &snapshot.head.roots,
        &object_commitments,
        &indices,
    )
    .map_err(internal_error)?;
    let head = snapshot.head;
    let state_root = head.current_state_root().ok_or_else(|| {
        (
            StatusCode::CONFLICT,
            "synchronizer has no canonical grounded state yet".to_string(),
        )
    })?;
    let state_root_hash = head
        .metadata
        .current_gsr
        .unwrap_or_else(|| state_root.hash());

    Ok(Json(GroundingWitnessResponse {
        state_root_hash,
        block_number: state_root.block_number,
        created_root: state_root.created_root,
        nullifiers_root: state_root.nullifiers_root,
        gsrs_root: state_root.gsrs_root,
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

fn build_head_snapshot(snapshot: &CurrentSnapshot) -> HeadSnapshot {
    HeadSnapshot {
        last_processed_slot: snapshot.last_processed_slot,
        last_processed_block_number: snapshot.last_processed_block_number,
        current_gsr: snapshot.head.metadata.current_gsr,
        current_block_number: snapshot.head.metadata.current_block_number.map(i64::from),
        created_count: snapshot.head.metadata.created_count as usize,
        nullifier_count: snapshot.head.metadata.nullifier_count as usize,
        gsr_count: snapshot.head.metadata.gsr_count as usize,
    }
}

fn membership_snapshot(
    app_db: &AppDb,
    roots: &CanonicalRoots,
    object_commitments: &[Hash],
    nullifiers: &[Hash],
    indices: &HashMap<Hash, i64>,
) -> anyhow::Result<MembershipSnapshot> {
    Ok(MembershipSnapshot {
        created_present: app_db.created_exists_for(roots, object_commitments, indices)?,
        nullifier_present: app_db.nullifier_exists_batch(roots, nullifiers)?,
    })
}

fn grounding_witness(
    app_db: &AppDb,
    roots: &CanonicalRoots,
    object_commitments: &[Hash],
    indices: &HashMap<Hash, i64>,
) -> anyhow::Result<GroundingWitnessSnapshot> {
    let witnesses = app_db.prove_created_for(roots, object_commitments, indices)?;
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
    let membership = membership_snapshot(
        &app_state.app_db,
        &snapshot.head.roots,
        &object_commitments,
        &nullifiers,
        &indices,
    )
    .map_err(internal_error)?;

    Ok(MembershipContext {
        head: build_head_snapshot(&snapshot),
        object_commitments,
        nullifiers,
        membership,
    })
}

async fn load_current_snapshot(
    app_state: &AppState,
) -> Result<CurrentSnapshot, (StatusCode, String)> {
    app_state
        .sync_db
        .current_snapshot()
        .await
        .map_err(internal_error)
}

async fn load_sync_progress(
    app_state: &AppState,
) -> Result<(u32, Option<u32>), (StatusCode, String)> {
    let snapshot = load_current_snapshot(app_state).await?;
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
