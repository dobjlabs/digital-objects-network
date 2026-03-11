use std::{net::SocketAddr, sync::Arc};

use anyhow::Result;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use hex::ToHex;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tracing::{debug, info};
use uuid::Uuid;

use common::{blob::MAX_SIMPLE_BLOB_PAYLOAD_BYTES, proof::BlobParser};

use crate::{
    db::{Db, InsertJobResult},
    model::{JobStatus, RelayJob},
};

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Db>,
    pub parser: Arc<dyn BlobParser>,
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
    BadRequest(String),
    NotFound(String),
    Internal(anyhow::Error),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
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
    bind_addr: SocketAddr,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<()> {
    let app_state = AppState { db, parser };

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
    Json(req): Json<SubmitProofRequest>,
) -> Result<(StatusCode, Json<SubmitProofResponse>), ApiError> {
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

    info!(
        payload_bytes = payload_bytes.len(),
        client_ref = ?req.client_ref,
        "Received proof submission"
    );

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
        .await
        .map_err(ApiError::Internal)?
    {
        info!(
            job_id = %existing.job_id,
            status = existing.status.as_str(),
            tx_final = %existing.tx_final,
            "Idempotent submission returned existing relay job"
        );
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
        .insert_job(&job)
        .await
        .map_err(ApiError::Internal)?
    {
        InsertJobResult::Inserted => {
            info!(
                job_id = %job.job_id,
                tx_final = %job.tx_final,
                payload_bytes = job.payload_bytes.len(),
                "Accepted new relay job"
            );
            (StatusCode::ACCEPTED, to_submit_response(job))
        }
        InsertJobResult::Existing(existing) => {
            info!(
                job_id = %existing.job_id,
                status = existing.status.as_str(),
                tx_final = %existing.tx_final,
                "Concurrent idempotent insert returned existing relay job"
            );
            (StatusCode::OK, to_submit_response(existing))
        }
    };

    Ok((status.0, Json(status.1)))
}

async fn get_proof(
    State(app_state): State<AppState>,
    Path(job_id): Path<String>,
) -> Result<Json<JobStatusResponse>, ApiError> {
    debug!(job_id = %job_id, "Handling relay job status request");

    let job = app_state
        .db
        .get_job(&job_id)
        .await
        .map_err(ApiError::Internal)?
        .ok_or_else(|| ApiError::NotFound(format!("job not found: {job_id}")))?;

    debug!(
        job_id = %job.job_id,
        status = job.status.as_str(),
        tx_hash = ?job.tx_hash,
        attempts = job.attempt_count,
        "Returning relay job status"
    );
    Ok(Json(to_status_response(job)))
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
    use sqlx::{Executor, postgres::PgPoolOptions};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Mutex, OnceLock};
    use std::time::{SystemTime, UNIX_EPOCH};
    use tower::ServiceExt;
    use url::Url;

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

    fn test_urls() -> (String, String, String) {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let admin_url = std::env::var("TEST_RELAYER_DB_ADMIN")
            .unwrap_or_else(|_| "postgres://postgres@localhost:5432/postgres".to_string());
        let mut url = Url::parse(&admin_url).expect("valid admin url");
        let db_name = format!(
            "relayer_api_test_{}_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        url.set_path(&format!("/{db_name}"));
        (admin_url, url.to_string(), db_name)
    }

    fn test_db_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    async fn drop_db(admin_url: &str, db_name: &str) -> anyhow::Result<()> {
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(admin_url)
            .await?;
        sqlx::query(
            "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = $1 AND pid <> pg_backend_pid()",
        )
        .bind(db_name)
        .execute(&pool)
        .await?;
        let escaped = db_name.replace('"', "\"\"");
        let stmt = format!("DROP DATABASE IF EXISTS \"{escaped}\"");
        pool.execute(stmt.as_str()).await?;
        Ok(())
    }

    async fn test_app(mode: ParseMode) -> (Router, String, String) {
        let (admin_url, db_url, db_name) = test_urls();
        drop_db(&admin_url, &db_name)
            .await
            .expect("drop test db before run");
        let db = Arc::new(Db::connect(&db_url).await.expect("connect test db"));
        let state = AppState {
            db,
            parser: Arc::new(MockParser { mode }),
        };
        (build_router(state), admin_url, db_name)
    }

    #[tokio::test]
    #[ignore = "requires local postgres"]
    async fn submit_rejects_invalid_base64() {
        let _guard = test_db_lock().lock().expect("lock");
        let (app, admin_url, db_name) = test_app(ParseMode::Valid).await;
        let req = Request::builder()
            .method("POST")
            .uri("/api/v1/proofs")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({"payload_base64": "!not-b64!"}).to_string(),
            ))
            .expect("request");

        let resp = app.clone().oneshot(req).await.expect("response");
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        drop(app);
        drop_db(&admin_url, &db_name)
            .await
            .expect("cleanup test db");
    }

    #[tokio::test]
    #[ignore = "requires local postgres"]
    async fn submit_rejects_oversize_payload() {
        let _guard = test_db_lock().lock().expect("lock");
        let (app, admin_url, db_name) = test_app(ParseMode::Valid).await;
        let payload = STANDARD.encode(vec![7u8; MAX_SIMPLE_BLOB_PAYLOAD_BYTES + 1]);
        let req = Request::builder()
            .method("POST")
            .uri("/api/v1/proofs")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({ "payload_base64": payload }).to_string(),
            ))
            .expect("request");

        let resp = app.clone().oneshot(req).await.expect("response");
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        drop(app);
        drop_db(&admin_url, &db_name)
            .await
            .expect("cleanup test db");
    }

    #[tokio::test]
    #[ignore = "requires local postgres"]
    async fn submit_rejects_invalid_payload_format() {
        let _guard = test_db_lock().lock().expect("lock");
        let (app, admin_url, db_name) = test_app(ParseMode::None).await;
        let req = Request::builder()
            .method("POST")
            .uri("/api/v1/proofs")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({ "payload_base64": STANDARD.encode([1u8, 2u8, 3u8]) })
                    .to_string(),
            ))
            .expect("request");

        let resp = app.clone().oneshot(req).await.expect("response");
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        drop(app);
        drop_db(&admin_url, &db_name)
            .await
            .expect("cleanup test db");
    }

    #[tokio::test]
    #[ignore = "requires local postgres"]
    async fn submit_rejects_invalid_proof() {
        let _guard = test_db_lock().lock().expect("lock");
        let (app, admin_url, db_name) = test_app(ParseMode::Err).await;
        let req = Request::builder()
            .method("POST")
            .uri("/api/v1/proofs")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({ "payload_base64": STANDARD.encode([1u8, 2u8, 3u8]) })
                    .to_string(),
            ))
            .expect("request");

        let resp = app.clone().oneshot(req).await.expect("response");
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        drop(app);
        drop_db(&admin_url, &db_name)
            .await
            .expect("cleanup test db");
    }

    #[tokio::test]
    #[ignore = "requires local postgres"]
    async fn submit_is_idempotent_by_tx_final() {
        let _guard = test_db_lock().lock().expect("lock");
        let (app, admin_url, db_name) = test_app(ParseMode::Valid).await;
        let payload = STANDARD.encode([1u8, 2u8, 3u8]);
        let body = serde_json::json!({"payload_base64": payload}).to_string();

        let req1 = Request::builder()
            .method("POST")
            .uri("/api/v1/proofs")
            .header("content-type", "application/json")
            .body(Body::from(body.clone()))
            .expect("request");
        let req2 = Request::builder()
            .method("POST")
            .uri("/api/v1/proofs")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .expect("request");

        let resp1 = app.clone().oneshot(req1).await.expect("response");
        assert_eq!(resp1.status(), StatusCode::ACCEPTED);
        let bytes1 = axum::body::to_bytes(resp1.into_body(), usize::MAX)
            .await
            .expect("body");
        let first: SubmitProofResponse = serde_json::from_slice(&bytes1).expect("json");

        let resp2 = app.clone().oneshot(req2).await.expect("response");
        assert_eq!(resp2.status(), StatusCode::OK);
        let bytes2 = axum::body::to_bytes(resp2.into_body(), usize::MAX)
            .await
            .expect("body");
        let second: SubmitProofResponse = serde_json::from_slice(&bytes2).expect("json");
        let second_json: JsonValue = serde_json::from_slice(&bytes2).expect("json");

        assert_eq!(first.job_id, second.job_id);
        assert_eq!(first.tx_final, second.tx_final);
        assert!(second_json.get("tx_final").is_some());
        assert!(second_json.get("state_root_hash").is_some());
        assert!(second_json.get("attempt_count").is_some());
        assert!(second_json.get("txFinal").is_none());

        drop(app);
        drop_db(&admin_url, &db_name)
            .await
            .expect("cleanup test db");
    }
}
