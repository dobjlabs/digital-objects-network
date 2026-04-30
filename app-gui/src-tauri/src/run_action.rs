//! `run_action` Tauri command — drives `Driver::execute_named` and emits
//! progress events the frontend listens to via the `run-action-progress`
//! channel.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use tauri::Emitter;

use crate::error::CommandError;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunActionInput {
    /// Action name as it appears in `driver::all_actions()` (e.g. "FindLog").
    /// Field is named `action_id` for compatibility with the existing
    /// frontend payload shape (`{ actionId: "FindLog", ... }`); semantically
    /// it's the action name, used to look up the dispatcher key.
    pub action_id: String,
    /// Either an absolute path to a `.dobj` file or a bare file name in the
    /// objects dir. The handler normalises to the bare name.
    pub input_object_paths: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunActionResult {
    pub ok: bool,
    pub action_id: String,
    pub tx_final: String,
    pub state_root_hash: String,
    /// Old root before the action; the new stack doesn't carry an explicit
    /// "old root" — surface the same `state_root_hash` for both fields so
    /// the frontend's old-pod2 shape keeps rendering.
    pub old_root: String,
    pub new_root: String,
    pub output_files: Vec<String>,
    pub nullified_files: Vec<String>,
    pub tx_hash: Option<String>,
    pub block_number: Option<u64>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct RunActionProgress {
    action_id: String,
    phase: &'static str,
    status: &'static str,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    tx_final: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_files: Option<Vec<String>>,
}

const PROGRESS_EVENT: &str = "run-action-progress";

#[tauri::command]
pub async fn run_action(
    app: tauri::AppHandle,
    driver: tauri::State<'_, Arc<::driver::Driver>>,
    input: RunActionInput,
) -> Result<RunActionResult, CommandError> {
    let driver = driver.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        emit(
            &app,
            RunActionProgress {
                action_id: input.action_id.clone(),
                phase: "generateProof",
                status: "running",
                message: "Generating proof".to_string(),
                tx_final: None,
                output_files: None,
            },
        );

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
                Ok(::driver::driver::ObjectSelector::FileName(file_name))
            })
            .collect::<Result<Vec<_>>>()?;

        let result = driver.execute_named(&input.action_id, input_objects)?;

        emit(
            &app,
            RunActionProgress {
                action_id: input.action_id.clone(),
                phase: "commit",
                status: "done",
                message: "Done".to_string(),
                tx_final: Some(result.tx_final.clone()),
                output_files: Some(result.output_files.clone()),
            },
        );

        Ok::<_, anyhow::Error>(RunActionResult {
            ok: true,
            action_id: result.action_name.to_string(),
            old_root: result.state_root_hash.clone(),
            new_root: result.state_root_hash.clone(),
            tx_final: result.tx_final,
            state_root_hash: result.state_root_hash,
            output_files: result.output_files,
            nullified_files: result.nullified_files,
            tx_hash: result.tx_hash,
            block_number: result.block_number,
        })
    })
    .await
    .map_err(|err| anyhow!("failed to run action task: {err}"))?
    .map_err(Into::into)
}

fn emit(app: &tauri::AppHandle, payload: RunActionProgress) {
    let _ = app.emit(PROGRESS_EVENT, payload);
}
