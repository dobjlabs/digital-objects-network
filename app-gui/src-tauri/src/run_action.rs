use std::path::Path;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};

use crate::error::CommandError;
use crate::progress::TauriProgressReporter;

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

#[tauri::command]
pub async fn run_action(
    app: tauri::AppHandle,
    driver: tauri::State<'_, Arc<::driver::Driver>>,
    input: RunActionInput,
) -> Result<RunActionResult, CommandError> {
    let driver = driver.inner().clone();
    tauri::async_runtime::spawn_blocking(move || run_action_core(app, driver, input))
        .await
        .map_err(|err| anyhow!("failed to run action task: {err}"))?
        .map_err(Into::into)
}

pub(crate) fn run_action_core(
    app: tauri::AppHandle,
    driver: Arc<::driver::Driver>,
    input: RunActionInput,
) -> Result<RunActionResult> {
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
            Ok(::driver::ObjectSelector::FileName(file_name))
        })
        .collect::<Result<Vec<_>>>()?;

    let reporter = TauriProgressReporter::new(app, input.action_id.clone());
    let result = driver.execute_with_reporter(
        ::driver::ExecuteActionInput {
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
}
