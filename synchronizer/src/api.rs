use std::{net::SocketAddr, sync::Arc};

use anyhow::Result;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use pod2::{backends::plonky2::primitives::merkletree::MerkleProof, middleware::Hash};
use wire_types::synchronizer::{
    GroundingWitnessRequest, GroundingWitnessResponse, HealthResponse, MembershipRequest,
    MembershipResponse, NullifierContainsEntry, NullifierContainsRequest,
    NullifierContainsResponse, SourceTxProofResponse, StateHeadResponse, SyncProgressResponse,
    TxContainsEntry, TxContainsRequest, TxContainsResponse, TxStatusResponse,
};
use tokio::sync::watch;
use tracing::info;

use crate::{
    app_db::AppDb,
    head::CanonicalRoots,
    sync_db::{CurrentSnapshot, SyncDb},
};
use common::{decode_hash_hex, encode_hash_hex};

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
    /// Current canonical global state root encoded as hex, if one exists.
    current_gsr: Option<String>,
    /// Execution block number committed inside the current state root, if any.
    current_block_number: Option<i64>,
    /// Number of accepted transactions in canonical state.
    tx_count: usize,
    /// Number of spent nullifiers in canonical state.
    nullifier_count: usize,
    /// Number of GSR entries in canonical history.
    gsr_count: usize,
}

#[derive(Debug, Clone)]
/// Membership result anchored to one caller-provided root set.
struct MembershipSnapshot {
    /// Per-request transaction membership bits under `roots.transactions`.
    tx_present: Vec<bool>,
    /// Per-request nullifier membership bits under `roots.nullifiers`.
    nullifier_present: Vec<bool>,
}

#[derive(Debug, Clone)]
/// Membership proof for a source transaction against the current transactions set root.
struct TxMembershipProof {
    /// Source transaction hash the client asked about.
    tx_hash: Hash,
    /// Whether the transaction is present in the committed transactions set.
    present: bool,
    /// Merkle proof against the current transactions set root.
    proof: MerkleProof,
}

#[derive(Debug, Clone)]
/// Proof-bearing result used by txlib to ground action execution.
struct GroundingWitnessSnapshot {
    /// Per-source transaction membership proofs under the provided roots.
    source_tx_proofs: Vec<TxMembershipProof>,
}

struct MembershipContext {
    head: HeadSnapshot,
    tx_hashes: Vec<Hash>,
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
        .route("/v1/state/tx/contains", post(post_state_tx_contains))
        .route(
            "/v1/state/nullifier/contains",
            post(post_state_nullifier_contains),
        )
        .route("/v1/state/tx/{tx_hash}", get(get_state_tx))
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
        tx_count: head.tx_count,
        nullifier_count: head.nullifier_count,
        gsr_count: head.gsr_count,
    }))
}

async fn post_state_tx_contains(
    State(app_state): State<AppState>,
    Json(body): Json<TxContainsRequest>,
) -> Result<Json<TxContainsResponse>, (StatusCode, String)> {
    let no_nullifiers: &[String] = &[];
    let MembershipContext {
        head,
        tx_hashes,
        membership,
        ..
    } = load_membership_context(&app_state, &body.tx_hashes, no_nullifiers).await?;

    Ok(Json(TxContainsResponse {
        last_processed_slot: head.last_processed_slot,
        current_gsr: head.current_gsr,
        results: tx_contains_entries(tx_hashes, membership.tx_present),
    }))
}

async fn post_state_nullifier_contains(
    State(app_state): State<AppState>,
    Json(body): Json<NullifierContainsRequest>,
) -> Result<Json<NullifierContainsResponse>, (StatusCode, String)> {
    let no_tx_hashes: &[String] = &[];
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
        tx_hashes,
        nullifiers,
        membership,
    } = load_membership_context(&app_state, &body.tx_hashes, &body.nullifiers).await?;

    Ok(Json(MembershipResponse {
        last_processed_slot: head.last_processed_slot,
        current_gsr: head.current_gsr,
        tx_results: tx_contains_entries(tx_hashes, membership.tx_present),
        nullifier_results: nullifier_contains_entries(nullifiers, membership.nullifier_present),
    }))
}

async fn get_state_tx(
    State(app_state): State<AppState>,
    Path(tx_hash): Path<String>,
) -> Result<Json<TxStatusResponse>, (StatusCode, String)> {
    let snapshot = load_current_snapshot(&app_state).await?;
    let hash = parse_hash_hex(&tx_hash)?;
    let membership = membership_snapshot(
        &app_state.app_db,
        &snapshot.head.roots,
        std::slice::from_ref(&hash),
        &[],
    )
    .map_err(internal_error)?;
    let head = build_head_snapshot(&snapshot);
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
    ensure_hash_query_limit("sourceTxHashes", body.source_tx_hashes.len())?;
    let snapshot = load_current_snapshot(&app_state).await?;
    let source_tx_hashes = body
        .source_tx_hashes
        .iter()
        .map(|raw| parse_hash_hex(raw))
        .collect::<Result<Vec<_>, _>>()?;
    let witness = grounding_witness(&app_state.app_db, &snapshot.head.roots, &source_tx_hashes)
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
        state_root_hash: encode_hash_hex(&state_root_hash),
        block_number: state_root.block_number,
        transactions_root: encode_hash_hex(&state_root.transactions_root),
        nullifiers_root: encode_hash_hex(&state_root.nullifiers_root),
        gsrs_root: encode_hash_hex(&state_root.gsrs_root),
        source_tx_proofs: witness
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

fn build_head_snapshot(snapshot: &CurrentSnapshot) -> HeadSnapshot {
    HeadSnapshot {
        last_processed_slot: snapshot.last_processed_slot,
        last_processed_block_number: snapshot.last_processed_block_number,
        current_gsr: snapshot
            .head
            .metadata
            .current_gsr
            .as_ref()
            .map(encode_hash_hex),
        current_block_number: snapshot.head.metadata.current_block_number.map(i64::from),
        tx_count: snapshot.head.metadata.tx_count as usize,
        nullifier_count: snapshot.head.metadata.nullifier_count as usize,
        gsr_count: snapshot.head.metadata.gsr_count as usize,
    }
}

fn membership_snapshot(
    app_db: &AppDb,
    roots: &CanonicalRoots,
    tx_hashes: &[Hash],
    nullifiers: &[Hash],
) -> anyhow::Result<MembershipSnapshot> {
    Ok(MembershipSnapshot {
        tx_present: app_db.tx_exists_batch(roots, tx_hashes)?,
        nullifier_present: app_db.nullifier_exists_batch(roots, nullifiers)?,
    })
}

fn grounding_witness(
    app_db: &AppDb,
    roots: &CanonicalRoots,
    source_tx_hashes: &[Hash],
) -> anyhow::Result<GroundingWitnessSnapshot> {
    let source_tx_proofs = source_tx_hashes
        .iter()
        .map(|tx_hash| {
            let (present, proof) = app_db.prove_tx(roots, *tx_hash)?;
            Ok(TxMembershipProof {
                tx_hash: *tx_hash,
                present,
                proof,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(GroundingWitnessSnapshot { source_tx_proofs })
}

async fn load_membership_context(
    app_state: &AppState,
    tx_hashes: &[String],
    nullifiers: &[String],
) -> Result<MembershipContext, (StatusCode, String)> {
    ensure_membership_query_limit(tx_hashes.len(), nullifiers.len())?;
    let snapshot = load_current_snapshot(app_state).await?;
    let tx_hashes = parse_hashes(tx_hashes)?;
    let nullifiers = parse_hashes(nullifiers)?;
    let membership = membership_snapshot(
        &app_state.app_db,
        &snapshot.head.roots,
        &tx_hashes,
        &nullifiers,
    )
    .map_err(internal_error)?;

    Ok(MembershipContext {
        head: build_head_snapshot(&snapshot),
        tx_hashes,
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

fn parse_hashes(values: &[String]) -> Result<Vec<Hash>, (StatusCode, String)> {
    values.iter().map(|value| parse_hash_hex(value)).collect()
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

fn tx_contains_entries(tx_hashes: Vec<Hash>, tx_present: Vec<bool>) -> Vec<TxContainsEntry> {
    tx_hashes
        .into_iter()
        .zip(tx_present)
        .map(|(hash, present)| TxContainsEntry {
            tx_hash: encode_hash_hex(&hash),
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
            nullifier: encode_hash_hex(&hash),
            present,
        })
        .collect()
}

fn parse_hash_hex(value: &str) -> Result<Hash, (StatusCode, String)> {
    decode_hash_hex(value.trim()).map_err(|err| (StatusCode::BAD_REQUEST, err.to_string()))
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
