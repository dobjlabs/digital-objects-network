use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use wire_types::{ProofJobState, RunActionInput, RunListFilter, RunListQuery, WaitQuery};

use crate::error::ApiResult;
use crate::state::AppState;

/// `POST /proofs`: start a proof job. Follow completion through
/// `GET /proofs/{jobId}` or `/events`; input errors appear as job failures.
pub async fn start(
    State(state): State<AppState>,
    Json(input): Json<RunActionInput>,
) -> (StatusCode, Json<ProofJobState>) {
    let job = crate::jobs::spawn_proof(&state.services, input);
    (StatusCode::ACCEPTED, Json(job))
}

/// `GET /proofs`: every job dobjd still holds, oldest first.
/// `?status=active` keeps only the unfinished ones.
pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<RunListQuery>,
) -> Json<Vec<ProofJobState>> {
    let active_only = query.status == Some(RunListFilter::Active);
    Json(
        state
            .services
            .jobs
            .entries(active_only)
            .iter()
            .map(|entry| entry.snapshot())
            .collect(),
    )
}

/// `GET /proofs/{jobId}`: return the job state, optionally waiting for
/// completion or timeout.
pub async fn get(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
    Query(query): Query<WaitQuery>,
) -> ApiResult<Json<ProofJobState>> {
    if state.services.jobs.get(&job_id).is_none() {
        return Err(crate::error::ApiError::new(
            StatusCode::NOT_FOUND,
            format!("unknown job: {job_id}"),
        ));
    }
    Ok(Json(state.services.wait_for_proof(&job_id, query).await?))
}
