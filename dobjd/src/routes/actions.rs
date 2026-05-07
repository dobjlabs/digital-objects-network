use std::path::Path as FsPath;

use anyhow::{Result, anyhow};
use axum::{
    Json,
    extract::{Path, State},
};
use driver::CheckActionReport;
use serde::{Deserialize, Serialize};

use crate::error::ApiResult;
use crate::progress::SseProgressReporter;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunActionInput {
    pub action_id: String,
    pub input_object_paths: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunActionResult {
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

    let result = tokio::task::spawn_blocking(move || -> Result<RunActionResult> {
        let input_objects = input
            .input_object_paths
            .iter()
            .map(|path| {
                let path = path.trim();
                let file_name = if FsPath::new(path).is_absolute() {
                    FsPath::new(path)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .ok_or_else(|| anyhow!("invalid input path: {path}"))?
                        .to_string()
                } else {
                    path.to_string()
                };
                Ok(driver::ObjectSelector::FileName(file_name))
            })
            .collect::<Result<Vec<_>>>()?;

        let reporter = SseProgressReporter::new(events, input.action_id.clone());
        let result = driver.execute_with_reporter(
            driver::ExecuteActionInput {
                action_id: input.action_id,
                input_objects,
            },
            &reporter,
        )?;

        Ok(RunActionResult {
            old_root: result.old_root,
            new_root: result.new_root,
            output_files: result.output_files,
            nullified_files: result.nullified_files,
        })
    })
    .await
    .map_err(|err| anyhow!("run_action task panicked: {err}"))??;

    Ok(Json(result))
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
