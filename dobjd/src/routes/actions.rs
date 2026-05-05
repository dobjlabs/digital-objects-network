use std::path::Path;

use anyhow::{Result, anyhow};
use axum::{Json, extract::State};
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
    pub ok: bool,
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
                let file_name = if Path::new(path).is_absolute() {
                    Path::new(path)
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
            ok: true,
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
