//! Beacon REST shim over a local anvil devnet. See README.md for the projection
//! it implements and why each endpoint exists.

use std::{
    collections::HashMap,
    convert::Infallible,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use alloy::{
    consensus::Bytes48,
    eips::{
        eip1898::BlockId as ExecutionBlockId,
        eip4844::{c_kzg, env_settings::EnvKzgSettings, HeapBlob},
    },
    network::Ethereum,
    primitives::B256,
    providers::{Provider, RootProvider},
    rpc::types::{Block, BlockNumberOrTag},
};
use anyhow::{anyhow, Context, Result};
use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::get,
    Json, Router,
};
use axum_extra::extract::Query;
use bytes::Bytes;
use eth_clients::beacon::types::BlockId;
use futures_util::stream::Stream;
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::{info, warn};
use wire_types::HealthResponse;

const DEFAULT_BIND: &str = "127.0.0.1:8555";
const DEFAULT_ANVIL_URL: &str = "http://127.0.0.1:8545";
/// Poll interval for the head event stream. The synchronizer treats these
/// events as hints and re-reads the head anyway, so this only bounds latency.
const HEAD_POLL_INTERVAL: Duration = Duration::from_millis(500);
const ANVIL_CONNECT_ATTEMPTS: u32 = 120;
const ANVIL_CONNECT_INTERVAL: Duration = Duration::from_millis(500);

/// Commitments are a pure function of a block's blobs, and the archiver and the
/// synchronizer each ask for the same block once per slot, so without this the
/// blob fetch and the KZG pass both run twice per blob-bearing block.
type CommitmentCache = Arc<Mutex<HashMap<B256, Vec<Bytes48>>>>;

#[derive(Clone)]
struct AppState {
    rpc: RootProvider,
    anvil_url: String,
    http: reqwest::Client,
    network_id: u64,
    commitments: CommitmentCache,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let bind: SocketAddr = std::env::var("BEACON_SHIM_BIND")
        .unwrap_or_else(|_| DEFAULT_BIND.to_string())
        .parse()
        .context("invalid BEACON_SHIM_BIND")?;
    let anvil_url =
        std::env::var("ANVIL_RPC_URL").unwrap_or_else(|_| DEFAULT_ANVIL_URL.to_string());

    let rpc =
        RootProvider::<Ethereum>::new_http(anvil_url.parse().context("invalid ANVIL_RPC_URL")?);
    let network_id = wait_for_anvil(&rpc, &anvil_url).await?;

    // Building the trusted setup takes long enough to be visible, and it would
    // otherwise land inside the first request that carries a blob.
    tokio::task::spawn_blocking(|| {
        EnvKzgSettings::Default.get();
    });

    let state = AppState {
        rpc,
        anvil_url: anvil_url.trim_end_matches('/').to_string(),
        http: reqwest::Client::builder()
            .timeout(Duration::from_secs(8))
            .build()?,
        network_id,
        commitments: CommitmentCache::default(),
    };

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/eth/v1/config/spec", get(get_spec))
        .route("/eth/v1/beacon/headers/{block_id}", get(get_header))
        .route("/eth/v2/beacon/blocks/{block_id}", get(get_block))
        .route("/eth/v1/beacon/blobs/{block_id}", get(get_blobs))
        .route("/eth/v1/events", get(head_events))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(bind).await?;
    info!(%bind, %anvil_url, network_id, "Beacon shim listening");
    axum::serve(listener, app).await?;
    Ok(())
}

/// Started as a sibling process of anvil rather than after it, so treat a
/// refused connection as "not up yet" instead of a fatal misconfiguration.
async fn wait_for_anvil(rpc: &RootProvider, url: &str) -> Result<u64> {
    for _ in 1..ANVIL_CONNECT_ATTEMPTS {
        if let Ok(network_id) = rpc.get_chain_id().await {
            return Ok(network_id);
        }
        tokio::time::sleep(ANVIL_CONNECT_INTERVAL).await;
    }
    rpc.get_chain_id()
        .await
        .with_context(|| format!("anvil not reachable at {url}"))
}

async fn healthz() -> Json<HealthResponse> {
    Json(HealthResponse::stamped("dev", "devtools"))
}

async fn get_spec(State(state): State<AppState>) -> Json<Value> {
    Json(json!({ "data": { "DEPOSIT_NETWORK_ID": state.network_id.to_string() } }))
}

async fn get_header(
    Path(block_id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiError> {
    let block = resolve_block(&state, &block_id).await?;
    Ok(Json(json!({
        "data": {
            "root": block.header.hash,
            "header": {
                "message": {
                    "parent_root": block.header.parent_hash,
                    "slot": block.header.number.to_string(),
                }
            }
        }
    })))
}

async fn get_block(
    Path(block_id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiError> {
    let block = resolve_block(&state, &block_id).await?;
    let commitments = kzg_commitments(&state, &block).await?;
    Ok(Json(json!({
        "data": {
            "message": {
                "slot": block.header.number.to_string(),
                "parent_root": block.header.parent_hash,
                "body": {
                    "execution_payload": {
                        "block_hash": block.header.hash,
                        "block_number": block.header.number.to_string(),
                        "timestamp": block.header.timestamp.to_string(),
                    },
                    "blob_kzg_commitments": commitments,
                }
            }
        }
    })))
}

#[derive(Deserialize)]
struct BlobsQuery {
    #[serde(default)]
    versioned_hashes: Vec<B256>,
}

async fn get_blobs(
    Path(block_id): Path<String>,
    Query(query): Query<BlobsQuery>,
    State(state): State<AppState>,
) -> Result<Response, ApiError> {
    let body = fetch_blobs(&state, &block_id, &query.versioned_hashes).await?;
    Ok(([(header::CONTENT_TYPE, "application/json")], body).into_response())
}

async fn head_events(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let stream = futures_util::stream::unfold((state, 0u64), |(state, last_seen)| async move {
        loop {
            tokio::time::sleep(HEAD_POLL_INTERVAL).await;
            match state
                .rpc
                .get_block_by_number(BlockNumberOrTag::Latest)
                .await
            {
                Ok(Some(block)) if block.header.number > last_seen => {
                    let number = block.header.number;
                    let event = Event::default().event("head").data(
                        json!({
                            "slot": number.to_string(),
                            "block": block.header.hash,
                        })
                        .to_string(),
                    );
                    return Some((Ok(event), (state, number)));
                }
                Ok(_) => {}
                Err(err) => warn!(?err, "Head poll against anvil failed"),
            }
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// `head` and `finalized` both resolve to anvil's latest block: a devnet with
/// instant, non-reverting finality has no distinction to model.
async fn resolve_block(state: &AppState, block_id: &str) -> Result<Block, ApiError> {
    let block_id: BlockId = block_id.parse().map_err(|err: String| anyhow!(err))?;
    let block = match block_id {
        BlockId::Head | BlockId::Finalized => {
            state
                .rpc
                .get_block_by_number(BlockNumberOrTag::Latest)
                .await
        }
        BlockId::Hash(root) => {
            state
                .rpc
                .get_block(ExecutionBlockId::Hash(root.into()))
                .await
        }
        BlockId::Slot(slot) => {
            state
                .rpc
                .get_block_by_number(BlockNumberOrTag::Number(slot.into()))
                .await
        }
    };
    block
        .map_err(anyhow::Error::from)?
        .ok_or(ApiError::NotFound)
}

/// Recomputed from the blob bytes because anvil exposes blobs but not their
/// commitments. Consumers derive versioned hashes from these and index blobs by
/// the resulting position, so a placeholder would not just mismatch, it would
/// panic on the lookup.
async fn kzg_commitments(state: &AppState, block: &Block) -> Result<Vec<Bytes48>> {
    if block.header.blob_gas_used.unwrap_or(0) == 0 {
        return Ok(Vec::new());
    }
    let root = block.header.hash;
    if let Some(cached) = state
        .commitments
        .lock()
        .expect("cache not poisoned")
        .get(&root)
    {
        return Ok(cached.clone());
    }

    let body = fetch_blobs(state, &format!("{root:#x}"), &[]).await?;
    let blobs: Vec<HeapBlob> = serde_json::from_slice::<BlobsBody>(&body)?.data;
    let commitments = tokio::task::spawn_blocking(move || {
        let settings = EnvKzgSettings::Default.get();
        blobs
            .iter()
            .map(|blob| {
                let blob = c_kzg::Blob::from_bytes(blob.inner())?;
                settings
                    .blob_to_kzg_commitment(&blob)
                    .map(|commitment| Bytes48::from(*commitment.to_bytes()))
            })
            .collect::<Result<Vec<_>, c_kzg::Error>>()
            .map_err(|err| anyhow!("failed to compute kzg commitments: {err}"))
    })
    .await??;

    state
        .commitments
        .lock()
        .expect("cache not poisoned")
        .insert(root, commitments.clone());
    Ok(commitments)
}

#[derive(Deserialize)]
struct BlobsBody {
    data: Vec<HeapBlob>,
}

/// Returned as raw bytes: anvil's body already satisfies the caller's schema
/// (which ignores the two extra fields), and each blob in it is 128 KiB.
///
/// anvil parses `versioned_hashes` as one comma-separated value while
/// `eth-clients` sends repeated query pairs, which would reach anvil as the
/// last pair alone and silently return the wrong blob set.
async fn fetch_blobs(state: &AppState, block_id: &str, versioned_hashes: &[B256]) -> Result<Bytes> {
    let mut url = format!("{}/eth/v1/beacon/blobs/{}", state.anvil_url, block_id);
    if !versioned_hashes.is_empty() {
        let joined: Vec<String> = versioned_hashes.iter().map(|h| h.to_string()).collect();
        url.push_str(&format!("?versioned_hashes={}", joined.join(",")));
    }
    let response = state.http.get(&url).send().await?;
    if response.status() == StatusCode::NOT_FOUND {
        return Ok(Bytes::from_static(br#"{"data":[]}"#));
    }
    Ok(response.error_for_status()?.bytes().await?)
}

enum ApiError {
    /// Callers read 404 as "this slot holds no block" and skip it, so a missing
    /// block must not surface as a server error.
    NotFound,
    Internal(anyhow::Error),
}

impl From<anyhow::Error> for ApiError {
    fn from(err: anyhow::Error) -> Self {
        Self::Internal(err)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            Self::NotFound => (StatusCode::NOT_FOUND, "not found").into_response(),
            Self::Internal(err) => {
                warn!(?err, "Beacon shim request failed");
                (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response()
            }
        }
    }
}
