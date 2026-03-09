use std::{net::SocketAddr, sync::Arc};

use anyhow::Result;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use hex::ToHex;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tracing::info;
use uuid::Uuid;

use common::{blob_codec::MAX_SIMPLE_BLOB_PAYLOAD_BYTES, proof::BlobParser};

use crate::{
    auth::is_authorized,
    db::{Db, InsertJobResult},
    model::{JobStatus, RelayJob},
};

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Db>,
    pub parser: Arc<dyn BlobParser>,
    pub api_key: String,
}

#[derive(Debug, Deserialize)]
pub struct SubmitProofRequest {
    pub payload_base64: String,
    pub client_ref: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SubmitProofResponse {
    pub job_id: String,
    pub status: JobStatus,
    pub tx_final: String,
    pub state_root_hash: String,
    pub attempt_count: u32,
    pub created_at: i64,
}

#[derive(Debug, Serialize)]
pub struct JobStatusResponse {
    pub job_id: String,
    pub status: JobStatus,
    pub tx_hash: Option<String>,
    pub block_number: Option<u64>,
    pub attempt_count: u32,
    pub last_error: Option<String>,
    pub updated_at: i64,
    pub created_at: i64,
    pub tx_final: String,
    pub state_root_hash: String,
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    ok: bool,
}

pub enum ApiError {
    Unauthorized,
    BadRequest(String),
    NotFound(String),
    Internal(anyhow::Error),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            ApiError::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "unauthorized"})),
            )
                .into_response(),
            ApiError::BadRequest(msg) => (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": msg})),
            )
                .into_response(),
            ApiError::NotFound(msg) => (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": msg})),
            )
                .into_response(),
            ApiError::Internal(err) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": err.to_string()})),
            )
                .into_response(),
        }
    }
}

pub async fn run_api_server(
    db: Arc<Db>,
    parser: Arc<dyn BlobParser>,
    api_key: String,
    bind_addr: SocketAddr,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<()> {
    let app_state = AppState {
        db,
        parser,
        api_key,
    };

    let app = build_router(app_state);
    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    info!(%bind_addr, "Relayer API listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            loop {
                if *shutdown_rx.borrow() {
                    break;
                }
                if shutdown_rx.changed().await.is_err() {
                    break;
                }
            }
        })
        .await?;

    Ok(())
}

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/api/v1/proofs", post(submit_proof))
        .route("/api/v1/proofs/{job_id}", get(get_proof))
        .with_state(state)
}

async fn healthz() -> Json<HealthResponse> {
    Json(HealthResponse { ok: true })
}

async fn submit_proof(
    State(app_state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<SubmitProofRequest>,
) -> Result<(StatusCode, Json<SubmitProofResponse>), ApiError> {
    ensure_auth(&headers, &app_state.api_key)?;

    let payload_bytes = STANDARD
        .decode(req.payload_base64.as_bytes())
        .map_err(|_| ApiError::BadRequest("payload_base64 is invalid base64".to_string()))?;

    if payload_bytes.len() > MAX_SIMPLE_BLOB_PAYLOAD_BYTES {
        return Err(ApiError::BadRequest(format!(
            "payload exceeds single-blob limit: {} > {}",
            payload_bytes.len(),
            MAX_SIMPLE_BLOB_PAYLOAD_BYTES
        )));
    }

    let payload = app_state
        .parser
        .parse_blob(&payload_bytes)
        .map_err(|err| ApiError::BadRequest(format!("payload verification failed: {err}")))?
        .ok_or_else(|| {
            ApiError::BadRequest("payload did not decode into a valid proof payload".to_string())
        })?;

    let tx_final = payload.tx_final.encode_hex::<String>();
    let state_root_hash = payload.state_root_hash.encode_hex::<String>();

    if let Some(existing) = app_state
        .db
        .get_job_by_tx_final(&tx_final)
        .map_err(ApiError::Internal)?
    {
        return Ok((StatusCode::OK, Json(to_submit_response(existing))));
    }

    let now = now_ts();
    let job = RelayJob {
        job_id: Uuid::new_v4().to_string(),
        status: JobStatus::Queued,
        payload_bytes,
        tx_final,
        state_root_hash,
        client_ref: req.client_ref,
        attempt_count: 0,
        tx_hash: None,
        submitted_at: None,
        block_number: None,
        last_error: None,
        next_attempt_at: Some(now),
        created_at: now,
        updated_at: now,
    };

    let status = match app_state
        .db
        .insert_job_idempotent(&job)
        .map_err(ApiError::Internal)?
    {
        InsertJobResult::Inserted => (StatusCode::ACCEPTED, to_submit_response(job)),
        InsertJobResult::Existing(existing) => (StatusCode::OK, to_submit_response(existing)),
    };

    Ok((status.0, Json(status.1)))
}

async fn get_proof(
    State(app_state): State<AppState>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> Result<Json<JobStatusResponse>, ApiError> {
    ensure_auth(&headers, &app_state.api_key)?;

    let job = app_state
        .db
        .get_job(&job_id)
        .map_err(ApiError::Internal)?
        .ok_or_else(|| ApiError::NotFound(format!("job not found: {job_id}")))?;

    Ok(Json(to_status_response(job)))
}

fn ensure_auth(headers: &HeaderMap, api_key: &str) -> Result<(), ApiError> {
    if is_authorized(headers, api_key) {
        Ok(())
    } else {
        Err(ApiError::Unauthorized)
    }
}

fn to_submit_response(job: RelayJob) -> SubmitProofResponse {
    SubmitProofResponse {
        job_id: job.job_id,
        status: job.status,
        tx_final: job.tx_final,
        state_root_hash: job.state_root_hash,
        attempt_count: job.attempt_count,
        created_at: job.created_at,
    }
}

fn to_status_response(job: RelayJob) -> JobStatusResponse {
    JobStatusResponse {
        job_id: job.job_id,
        status: job.status,
        tx_hash: job.tx_hash,
        block_number: job.block_number,
        attempt_count: job.attempt_count,
        last_error: job.last_error,
        updated_at: job.updated_at,
        created_at: job.created_at,
        tx_final: job.tx_final,
        state_root_hash: job.state_root_hash,
    }
}

fn now_ts() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use common::{payload::Payload, payload::PayloadProof};
    use pod2::middleware::EMPTY_HASH;
    use serde_json::Value as JsonValue;
    use tempfile::TempDir;
    use tower::ServiceExt;

    enum ParseMode {
        Valid,
        None,
        Err,
    }

    struct MockParser {
        mode: ParseMode,
    }

    impl BlobParser for MockParser {
        fn parse_blob(&self, _blob_bytes: &[u8]) -> anyhow::Result<Option<Payload>> {
            match self.mode {
                ParseMode::Valid => Ok(Some(Payload {
                    proof: PayloadProof::Groth16(vec![]),
                    tx_final: EMPTY_HASH,
                    state_root_hash: EMPTY_HASH,
                    nullifiers: vec![],
                })),
                ParseMode::None => Ok(None),
                ParseMode::Err => Err(anyhow::anyhow!("invalid proof")),
            }
        }
    }

    fn test_state(mode: ParseMode) -> AppState {
        let dir = TempDir::new().unwrap();
        let db = Arc::new(Db::connect(dir.path().to_str().unwrap()).unwrap());
        AppState {
            db,
            parser: Arc::new(MockParser { mode }),
            api_key: "test-key".to_string(),
        }
    }

    #[tokio::test]
    async fn submit_rejects_invalid_base64() {
        let app = build_router(test_state(ParseMode::Valid));
        let req = Request::builder()
            .method("POST")
            .uri("/api/v1/proofs")
            .header("authorization", "Bearer test-key")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({"payload_base64": "!not-b64!"}).to_string(),
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn submit_rejects_oversize_payload() {
        let app = build_router(test_state(ParseMode::Valid));
        let payload = STANDARD.encode(vec![7u8; MAX_SIMPLE_BLOB_PAYLOAD_BYTES + 1]);
        let req = Request::builder()
            .method("POST")
            .uri("/api/v1/proofs")
            .header("authorization", "Bearer test-key")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({ "payload_base64": payload }).to_string(),
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn submit_rejects_invalid_payload_format() {
        let app = build_router(test_state(ParseMode::None));
        let req = Request::builder()
            .method("POST")
            .uri("/api/v1/proofs")
            .header("authorization", "Bearer test-key")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({ "payload_base64": STANDARD.encode([1u8, 2u8, 3u8]) })
                    .to_string(),
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn submit_rejects_invalid_proof() {
        let app = build_router(test_state(ParseMode::Err));
        let req = Request::builder()
            .method("POST")
            .uri("/api/v1/proofs")
            .header("authorization", "Bearer test-key")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({ "payload_base64": STANDARD.encode([1u8, 2u8, 3u8]) })
                    .to_string(),
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn submit_is_idempotent_by_tx_final() {
        let app = build_router(test_state(ParseMode::Valid));
        let payload = STANDARD.encode([1u8, 2u8, 3u8]);
        let body = serde_json::json!({"payload_base64": payload}).to_string();

        let req1 = Request::builder()
            .method("POST")
            .uri("/api/v1/proofs")
            .header("authorization", "Bearer test-key")
            .header("content-type", "application/json")
            .body(Body::from(body.clone()))
            .unwrap();
        let req2 = Request::builder()
            .method("POST")
            .uri("/api/v1/proofs")
            .header("authorization", "Bearer test-key")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();

        let resp1 = app.clone().oneshot(req1).await.unwrap();
        assert_eq!(resp1.status(), StatusCode::ACCEPTED);
        let bytes1 = axum::body::to_bytes(resp1.into_body(), usize::MAX)
            .await
            .unwrap();
        let first: SubmitProofResponse = serde_json::from_slice(&bytes1).unwrap();

        let resp2 = app.oneshot(req2).await.unwrap();
        assert_eq!(resp2.status(), StatusCode::OK);
        let bytes2 = axum::body::to_bytes(resp2.into_body(), usize::MAX)
            .await
            .unwrap();
        let second: SubmitProofResponse = serde_json::from_slice(&bytes2).unwrap();
        let second_json: JsonValue = serde_json::from_slice(&bytes2).unwrap();

        assert_eq!(first.job_id, second.job_id);
        assert_eq!(first.tx_final, second.tx_final);
        assert!(second_json.get("tx_final").is_some());
        assert!(second_json.get("state_root_hash").is_some());
        assert!(second_json.get("attempt_count").is_some());
        assert!(second_json.get("txFinal").is_none());
    }
}
