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
use eth_clients::{
    beacon::types::{BlobsResponse, BlockHeader},
    common::ErrorResponse,
};
use tokio::sync::RwLock;
use wire_types::HealthResponse;

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
        .route("/eth/v1/beacon/blobs/{block_id}", get(get_blobs))
        .layer(tower_http::cors::CorsLayer::permissive())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    info!(%bind_addr, "API server listening");
    axum::serve(listener, app).await?;
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
        Err((StatusCode::TOO_EARLY, "no header yet".to_string()))
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
) -> Result<Json<BlobsResponse>, (StatusCode, Json<ErrorResponse>)> {
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
            return Err(error_response(
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

/// The synchronizer reads this endpoint through `eth-clients`, which parses
/// every non-404 body as an [`ErrorResponse`]. A bare string reaches it as a
/// deserialization failure with the real message dropped.
fn error_response(status: StatusCode, message: String) -> (StatusCode, Json<ErrorResponse>) {
    (status, Json(ErrorResponse::new(status.as_u16(), message)))
}

fn internal_error(err: anyhow::Error) -> (StatusCode, Json<ErrorResponse>) {
    error_response(StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;
    use eth_clients::common::ClientResponse;

    /// The synchronizer's `eth-clients` reader feeds every non-404 body to
    /// `ClientResponse`, so a body that misses its `Error` arm arrives as a
    /// serde failure instead of the message.
    #[tokio::test]
    async fn error_body_parses_as_a_client_error() {
        let response = internal_error(anyhow::anyhow!("disk gone")).into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let parsed: ClientResponse<serde_json::Value> =
            serde_json::from_slice(&body).expect("error body must be JSON");

        match parsed {
            ClientResponse::Error(err) => {
                assert_eq!(err.message.as_deref(), Some("disk gone"));
                assert_eq!(err.code.to_string(), "500");
            }
            _ => panic!("error body did not parse as ClientResponse::Error"),
        }
    }
}
