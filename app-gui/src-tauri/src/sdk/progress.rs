use serde::Serialize;
use tauri::Emitter;

use super::run_action::RunSdkActionResult;

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
    pub(super) output_file: Option<String>,
}

fn emit_progress(app: &tauri::AppHandle, payload: &RunSdkActionProgress) -> Result<(), String> {
    app.emit("run-sdk-action-progress", payload)
        .map_err(|err| format!("failed to emit run progress: {err}"))
}

fn emit_phase(
    app: &tauri::AppHandle,
    run_id: &str,
    phase: ProofPhase,
    status: ProofProgressStatus,
    message: String,
    old_root: Option<&str>,
    new_root: Option<&str>,
    output_file: Option<String>,
) -> Result<(), String> {
    emit_progress(
        app,
        &RunSdkActionProgress {
            run_id: run_id.to_string(),
            phase,
            status,
            message,
            old_root: old_root.map(|value| value.to_string()),
            new_root: new_root.map(|value| value.to_string()),
            output_file,
        },
    )
}

pub(super) fn emit_generate_proof_step(
    app: &tauri::AppHandle,
    run_id: &str,
    step_label: &str,
) -> Result<(), String> {
    emit_phase(
        app,
        run_id,
        ProofPhase::GenerateProof,
        ProofProgressStatus::Running,
        step_label.to_string(),
        None,
        None,
        None,
    )
}

pub(super) fn emit_generate_proof_done(app: &tauri::AppHandle, run_id: &str) -> Result<(), String> {
    emit_phase(
        app,
        run_id,
        ProofPhase::GenerateProof,
        ProofProgressStatus::Done,
        "Proof generation complete".to_string(),
        None,
        None,
        None,
    )
}

pub(super) fn emit_commit_step(
    app: &tauri::AppHandle,
    run_id: &str,
    step_label: &str,
    old_root: &str,
) -> Result<(), String> {
    emit_phase(
        app,
        run_id,
        ProofPhase::Commit,
        ProofProgressStatus::Running,
        step_label.to_string(),
        Some(old_root),
        None,
        None,
    )
}

pub(super) fn emit_commit_done(
    app: &tauri::AppHandle,
    run_id: &str,
    result: &RunSdkActionResult,
) -> Result<(), String> {
    emit_phase(
        app,
        run_id,
        ProofPhase::Commit,
        ProofProgressStatus::Done,
        "Commit complete".to_string(),
        Some(&result.old_root),
        Some(&result.new_root),
        result.output_files.first().cloned(),
    )
}
