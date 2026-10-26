use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use wire_types::{ProofJobState, RunActionInput, RunListFilter, RunListQuery, WaitQuery};

use crate::error::{ApiError, ApiResult};
use crate::events::Event;
use crate::routes::wait::wait_until;
use crate::state::AppState;

/// `POST /proofs`: start a proof job. Follow completion through
/// `GET /proofs/{jobId}` or `/events`; input errors appear as job failures.
pub async fn start(
    State(state): State<AppState>,
    Json(input): Json<RunActionInput>,
) -> (StatusCode, Json<ProofJobState>) {
    let input_objects: Vec<String> = input
        .input_object_paths
        .iter()
        .map(|path| path.trim().to_string())
        .collect();
    let job = crate::jobs::spawn_proof(&state.services, input.action, input_objects, input.args);
    (StatusCode::ACCEPTED, Json(job))
}

/// `GET /proofs`: every job dobjd still holds, oldest first.
/// `?status=active` keeps only the unfinished ones.
pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<RunListQuery>,
) -> Json<Vec<ProofJobState>> {
    let active_only = query.status == Some(RunListFilter::Active);
    Json(state.services.jobs.list(active_only))
}

/// `GET /proofs/{jobId}`: return the job state, optionally waiting for
/// completion or timeout.
pub async fn get(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
    Query(query): Query<WaitQuery>,
) -> ApiResult<Json<ProofJobState>> {
    let entry =
        state.services.jobs.get(&job_id).ok_or_else(|| {
            ApiError::new(StatusCode::NOT_FOUND, format!("unknown job: {job_id}"))
        })?;
    let job = wait_until(
        &state.services.events,
        query,
        || Ok(entry.snapshot()),
        |job| job.status.is_terminal(),
        |event| match event {
            Event::ProofJob(job) if job.job_id == job_id => Some(job.clone()),
            _ => None,
        },
    )
    .await?;
    Ok(Json(job))
}
