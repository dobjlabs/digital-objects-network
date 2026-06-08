use anyhow::{Result, anyhow};
use axum::{
    Json,
    extract::{Path, State},
};
use wire_types::{
    ActionSummary, CheckActionReport, QualifiedName, RunActionRequest, RunActionResult,
};

use crate::error::ApiResult;
use crate::progress::SseProgressReporter;
use crate::state::AppState;

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
                    action: input.action,
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
                old_root: common::encode_hash_hex(&result.old_root),
                new_root: common::encode_hash_hex(&result.new_root),
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
/// `id` is the canonical `plugin::name` form.
pub async fn inspect_action(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<ActionSummary>> {
    let driver = state.driver.clone();
    let qname = QualifiedName::parse(&id).map_err(|err| anyhow!("{err}"))?;
    let action = tokio::task::spawn_blocking(move || driver.get_action(&qname))
        .await
        .map_err(|err| anyhow!("get_action task panicked: {err}"))??;
    Ok(Json(action))
}

/// `GET /actions/{id}/feasibility` — does the local inventory have what
/// this action needs? Returns the report shape `Driver::check_action`
/// produces: `feasible` flag, the candidate objects we'd use, and any
/// missing input class names. `id` is the canonical `plugin::name` form.
pub async fn check_feasibility(
    State(state): State<AppState>,
    Path(action_id): Path<String>,
) -> ApiResult<Json<CheckActionReport>> {
    let driver = state.driver.clone();
    let qname = QualifiedName::parse(&action_id).map_err(|err| anyhow!("{err}"))?;
    let report = tokio::task::spawn_blocking(move || driver.check_action(&qname))
        .await
        .map_err(|err| anyhow!("check_feasibility task panicked: {err}"))??;
    Ok(Json(report))
}
