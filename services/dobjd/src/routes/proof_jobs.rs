use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use wire_types::{ActionInput, ProofJobState, RunListFilter, RunListQuery, WaitQuery};

use crate::error::ApiResult;
use crate::state::AppState;

/// `POST /proof-jobs`: start a proof job. Follow completion through
/// `GET /proof-jobs/{jobId}` or `/events`; input errors appear as job failures.
pub async fn start(
    State(state): State<AppState>,
    Json(input): Json<ActionInput>,
) -> (StatusCode, Json<ProofJobState>) {
    let job = crate::proof_jobs::spawn_proof_job(&state.services, input);
    (StatusCode::ACCEPTED, Json(job))
}

/// `GET /proof-jobs`: every job dobjd still holds, oldest first.
/// `?status=active` keeps only the unfinished ones.
pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<RunListQuery>,
) -> Json<Vec<ProofJobState>> {
    let active_only = query.status == Some(RunListFilter::Active);
    Json(state.services.list_proof_jobs(active_only))
}

/// `GET /proof-jobs/{jobId}`: return the job state, optionally waiting for
/// completion or timeout.
pub async fn get(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
    Query(query): Query<WaitQuery>,
) -> ApiResult<Json<ProofJobState>> {
    Ok(Json(
        state.services.wait_for_proof_job(&job_id, query).await?,
    ))
}
