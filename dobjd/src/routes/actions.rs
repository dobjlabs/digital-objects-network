use anyhow::{Result, anyhow};
use axum::{
    Json,
    extract::{Path, State},
};
use driver::{ActionSummary, CheckActionReport};
use serde::{Deserialize, Serialize};

use crate::error::ApiResult;
use crate::progress::SseProgressReporter;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunActionInput {
    pub action_id: String,
    pub input_object_paths: Vec<String>,
    /// Optional client-generated correlation id for filtering progress
    /// events. If omitted, dobjd generates a UUID v4 and returns it on
    /// the response — clients that don't care about correlation can ignore
    /// it; clients that subscribe to `/events` use it to scope progress
    /// events to their own run when multiple runs are in flight.
    pub run_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunActionResult {
    /// The correlation id used for `run-action-progress` events scoped to
    /// this call. Echoed from the request when the client supplied one,
    /// otherwise a freshly-minted UUID v4.
    pub run_id: String,
    pub old_root: String,
    pub new_root: String,
    pub output_files: Vec<String>,
    pub nullified_files: Vec<String>,
}

/// Wire shape mirrors the Tauri command: `{ "input": { ... } }`.
#[derive(Debug, Deserialize)]
pub struct RunActionRequest {
    pub input: RunActionInput,
}

pub async fn run_action(
    State(state): State<AppState>,
    Json(req): Json<RunActionRequest>,
) -> ApiResult<Json<RunActionResult>> {
    let driver = state.driver.clone();
    let events = state.events.clone();
    let input = req.input;

    // Pin the run id before the worker thread so we can return it on both
    // the success and (eventually) the error path. action_id is not unique
    // across concurrent runs, so progress events have to be keyed on this
    // instead.
    let run_id = input
        .run_id
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let result = tokio::task::spawn_blocking({
        let run_id = run_id.clone();
        move || -> Result<RunActionResult> {
            // Pass strings through verbatim — the driver extracts basenames
            // via `Path::file_name`, so an absolute path or a bare basename
            // resolve to the same managed file.
            let input_objects: Vec<String> = input
                .input_object_paths
                .iter()
                .map(|path| path.trim().to_string())
                .collect();

            // Reporter is created here (after input parsing) so a malformed
            // request returns 400 without ever opening a progress window.
            // Once it exists, every error path must call `commit_failed`
            // before returning, so SSE subscribers see a terminal event.
            let reporter = SseProgressReporter::new(events, run_id.clone());
            let result = match driver.execute_with_reporter(
                driver::ExecuteActionInput {
                    action_id: input.action_id,
                    input_objects,
                },
                &reporter,
            ) {
                Ok(result) => result,
                Err(err) => {
                    reporter.commit_failed(err.to_string());
                    return Err(err);
                }
            };

            Ok(RunActionResult {
                run_id,
                old_root: result.old_root,
                new_root: result.new_root,
                output_files: result.output_files,
                nullified_files: result.nullified_files,
            })
        }
    })
    .await
    .map_err(|err| anyhow!("run_action task panicked: {err}"))??;

    Ok(Json(result))
}

/// `GET /actions` — full catalog of every action the loaded plugins
/// declare. Pure local state; no synchronizer round-trip. Use this
/// instead of `/inventory` when you only need the action list.
pub async fn list_actions(State(state): State<AppState>) -> ApiResult<Json<Vec<ActionSummary>>> {
    let driver = state.driver.clone();
    let actions = tokio::task::spawn_blocking(move || driver.list_actions(None))
        .await
        .map_err(|err| anyhow!("list_actions task panicked: {err}"))??;
    Ok(Json(actions))
}

/// `GET /actions/{id}` — one action detail with predicate source.
pub async fn inspect_action(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<ActionSummary>> {
    let driver = state.driver.clone();
    let action = tokio::task::spawn_blocking(move || driver.get_action(&id))
        .await
        .map_err(|err| anyhow!("get_action task panicked: {err}"))??;
    Ok(Json(action))
}

/// `GET /actions/{id}/feasibility` — does the local inventory have what
/// this action needs? Returns the report shape `Driver::check_action`
/// produces: `feasible` flag, the candidate objects we'd use, and any
/// missing input class names.
pub async fn check_feasibility(
    State(state): State<AppState>,
    Path(action_id): Path<String>,
) -> ApiResult<Json<CheckActionReport>> {
    let driver = state.driver.clone();
    let report = tokio::task::spawn_blocking(move || driver.check_action(&action_id))
        .await
        .map_err(|err| anyhow!("check_feasibility task panicked: {err}"))??;
    Ok(Json(report))
}
