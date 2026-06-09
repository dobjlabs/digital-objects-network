use std::{net::SocketAddr, sync::Arc};

use alloy::primitives::{Address, B256};
use anyhow::Result;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use axum_extra::extract::Query; // Required because the `versioned_hashes` query in the blobs
                                // endpoint requires repetition to encode an array
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::node::Store;
use eth_clients::beacon::types::{BlobsResponse, BlockHeader};
use tokio::sync::RwLock;

#[derive(Clone)]
pub(crate) struct ApiState {
    pub(crate) config: Arc<Config>,
    pub(crate) store: Arc<Store>,
    pub(crate) header: Arc<RwLock<Option<BlockHeader>>>,
}

pub async fn run_api_server(state: ApiState, bind_addr: SocketAddr) -> Result<()> {
    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/header", get(get_header))
        .route("/config", get(get_config))
        .route("/v1/beacon/blobs/{block_id}", get(get_blobs))
        .layer(tower_http::cors::CorsLayer::permissive())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    info!(%bind_addr, "API server listening");
    axum::serve(listener, app).await?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Liveness response for the synchronizer HTTP server.
pub struct HealthResponse {
    /// Whether the server is up and responding.
    pub ok: bool,
}

async fn healthz() -> Json<HealthResponse> {
    Json(HealthResponse { ok: true })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub filter_address: Address,
}

async fn get_config(State(state): State<ApiState>) -> Json<Config> {
    let config = (*state.config).clone();
    Json(config)
}

async fn get_header(
    State(state): State<ApiState>,
) -> Result<Json<BlockHeader>, (StatusCode, String)> {
    let header = state.header.read().await.clone();
    if let Some(header) = header {
        Ok(Json(header))
    } else {
        Err((StatusCode::TOO_EARLY, format!("no header yet")))
    }
}

#[derive(Deserialize)]
struct BlobsQuery {
    versioned_hashes: Vec<B256>,
}

#[axum::debug_handler]
async fn get_blobs(
    Path(block_id): Path<B256>,
    Query(query): Query<BlobsQuery>,
    State(state): State<ApiState>,
) -> Result<Json<BlobsResponse>, (StatusCode, String)> {
    let block_blobs = state
        .store
        .load_blobs_disk(&block_id)
        .await
        .map_err(internal_error)?;
    let mut versioned_hashes = query.versioned_hashes;
    versioned_hashes.sort();
    versioned_hashes.dedup();
    let mut blobs = Vec::new();
    for vh in versioned_hashes {
        if let Some((index, _, blob)) = block_blobs.iter().find(|(_, vh0, _)| vh == *vh0) {
            blobs.push((*index, blob));
        } else {
            return Err((
                StatusCode::NOT_FOUND,
                format!(
                    "blob with versioned_hash {} not found in stored blobs from block {}",
                    vh, block_id
                ),
            ));
        }
    }
    blobs.sort_by_key(|(index, _)| *index);

    Ok(Json(BlobsResponse {
        data: blobs.into_iter().map(|(_, blob)| blob.clone()).collect(),
    }))
}

fn internal_error(err: anyhow::Error) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}
