use serde::Serialize;
use tauri::Emitter;

use anyhow::{anyhow, Result};
use driver::{ExecuteActionResult, ExecutionPhase, ExecutionReporter};

#[derive(Debug, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(super) enum ProofPhase {
    GenerateProof,
    Commit,
}

#[derive(Debug, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(super) enum ProofProgressStatus {
    Running,
    Done,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(super) struct RunSdkActionProgress {
    pub(super) run_id: String,
    pub(super) phase: ProofPhase,
    pub(super) status: ProofProgressStatus,
    pub(super) message: String,
    pub(super) old_root: Option<String>,
    pub(super) new_root: Option<String>,
    pub(super) output_files: Option<Vec<String>>,
}

fn emit_progress(app: &tauri::AppHandle, payload: &RunSdkActionProgress) -> Result<()> {
    app.emit("run-sdk-action-progress", payload)
        .map_err(|err| anyhow!("failed to emit run progress: {err}"))
}

pub(super) fn emit_generate_proof_step(
    app: &tauri::AppHandle,
    run_id: &str,
    step_label: &str,
) -> Result<()> {
    let payload = RunSdkActionProgress {
        run_id: run_id.to_string(),
        phase: ProofPhase::GenerateProof,
        status: ProofProgressStatus::Running,
        message: step_label.to_string(),
        old_root: None,
        new_root: None,
        output_files: None,
    };
    emit_progress(app, &payload)
}

pub(super) fn emit_generate_proof_done(app: &tauri::AppHandle, run_id: &str) -> Result<()> {
    let payload = RunSdkActionProgress {
        run_id: run_id.to_string(),
        phase: ProofPhase::GenerateProof,
        status: ProofProgressStatus::Done,
        message: "Proof generation complete".to_string(),
        old_root: None,
        new_root: None,
        output_files: None,
    };
    emit_progress(app, &payload)
}

pub(super) fn emit_commit_done(
    app: &tauri::AppHandle,
    run_id: &str,
    result: &ExecuteActionResult,
) -> Result<()> {
    let payload = RunSdkActionProgress {
        run_id: run_id.to_string(),
        phase: ProofPhase::Commit,
        status: ProofProgressStatus::Done,
        message: "Commit complete".to_string(),
        old_root: Some(result.old_root.clone()),
        new_root: Some(result.new_root.clone()),
        output_files: Some(result.output_files.clone()),
    };
    emit_progress(app, &payload)
}

pub(crate) struct TauriProgressReporter {
    app: tauri::AppHandle,
    run_id: String,
}

impl TauriProgressReporter {
    pub(crate) fn new(app: tauri::AppHandle, run_id: String) -> Self {
        Self { app, run_id }
    }
}

impl ExecutionReporter for TauriProgressReporter {
    fn on_step(&self, phase: ExecutionPhase, message: &str) {
        let _ = match phase {
            ExecutionPhase::GenerateProof => {
                emit_generate_proof_step(&self.app, &self.run_id, message)
            }
            ExecutionPhase::Commit => {
                let payload = RunSdkActionProgress {
                    run_id: self.run_id.clone(),
                    phase: ProofPhase::Commit,
                    status: ProofProgressStatus::Running,
                    message: message.to_string(),
                    old_root: None,
                    new_root: None,
                    output_files: None,
                };
                emit_progress(&self.app, &payload)
            }
        };
    }

    fn on_done(&self, phase: ExecutionPhase, result: Option<&ExecuteActionResult>) {
        let _ = match phase {
            ExecutionPhase::GenerateProof => emit_generate_proof_done(&self.app, &self.run_id),
            ExecutionPhase::Commit => result
                .map(|result| emit_commit_done(&self.app, &self.run_id, result))
                .unwrap_or(Ok(())),
        };
    }
}
