use tauri::Emitter;

use crate::types::{RunSdkActionProgress, RunSdkActionResult};

use super::relayer_client::RelayerJobStatus;

fn emit_progress(app: &tauri::AppHandle, payload: &RunSdkActionProgress) -> Result<(), String> {
    app.emit("run-sdk-action-progress", payload)
        .map_err(|err| format!("failed to emit run progress: {err}"))
}

fn emit_phase(
    app: &tauri::AppHandle,
    run_id: &str,
    phase: &str,
    status: &str,
    message: String,
    verify_index: Option<usize>,
    detail: Option<String>,
    old_root: Option<&str>,
    new_root: Option<&str>,
    output_file: Option<String>,
) -> Result<(), String> {
    emit_progress(
        app,
        &RunSdkActionProgress {
            run_id: run_id.to_string(),
            phase: phase.to_string(),
            status: status.to_string(),
            message,
            verify_index,
            detail,
            old_root: old_root.map(|value| value.to_string()),
            new_root: new_root.map(|value| value.to_string()),
            output_file,
        },
    )
}

pub(super) fn emit_hash_running(
    app: &tauri::AppHandle,
    run_id: &str,
    action_id: &str,
    cpu_cost: &str,
) -> Result<(), String> {
    emit_phase(
        app,
        run_id,
        "hash",
        "running",
        format!("Running {action_id}"),
        None,
        Some(cpu_cost.to_string()),
        None,
        None,
        None,
    )
}

pub(super) fn emit_hash_done(
    app: &tauri::AppHandle,
    run_id: &str,
    cpu_cost: &str,
) -> Result<(), String> {
    emit_phase(
        app,
        run_id,
        "hash",
        "done",
        "Proof generation complete".to_string(),
        None,
        Some(cpu_cost.to_string()),
        None,
        None,
        None,
    )
}

pub(super) fn emit_verify_progress(
    app: &tauri::AppHandle,
    run_id: &str,
    verify_targets: &[String],
) -> Result<(), String> {
    if verify_targets.is_empty() {
        let placeholder = "(no inputs)";
        emit_phase(
            app,
            run_id,
            "verify",
            "running",
            format!("Verifying {placeholder}"),
            Some(0),
            Some(placeholder.to_string()),
            None,
            None,
            None,
        )?;
        emit_phase(
            app,
            run_id,
            "verify",
            "done",
            format!("Verified {placeholder}"),
            Some(0),
            Some(placeholder.to_string()),
            None,
            None,
            None,
        )?;
        return Ok(());
    }

    for (index, target) in verify_targets.iter().enumerate() {
        emit_phase(
            app,
            run_id,
            "verify",
            "running",
            format!("Verifying {target}"),
            Some(index),
            Some(target.clone()),
            None,
            None,
            None,
        )?;
        emit_phase(
            app,
            run_id,
            "verify",
            "done",
            format!("Verified {target}"),
            Some(index),
            Some(target.clone()),
            None,
            None,
            None,
        )?;
    }

    Ok(())
}

pub(super) fn emit_nullify_running(
    app: &tauri::AppHandle,
    run_id: &str,
    old_root: &str,
) -> Result<(), String> {
    emit_phase(
        app,
        run_id,
        "nullify",
        "running",
        format!("Nullifying {old_root}"),
        None,
        Some(old_root.to_string()),
        Some(old_root),
        None,
        None,
    )
}

pub(super) fn emit_nullify_done(
    app: &tauri::AppHandle,
    run_id: &str,
    old_root: &str,
) -> Result<(), String> {
    emit_phase(
        app,
        run_id,
        "nullify",
        "done",
        "Nullify complete".to_string(),
        None,
        Some(old_root.to_string()),
        Some(old_root),
        None,
        None,
    )
}

pub(super) fn emit_commit_submitting(
    app: &tauri::AppHandle,
    run_id: &str,
    old_root: &str,
) -> Result<(), String> {
    emit_phase(
        app,
        run_id,
        "commit",
        "running",
        "Submitting proof to relayer".to_string(),
        None,
        Some("submit".to_string()),
        Some(old_root),
        None,
        None,
    )
}

pub(super) fn emit_commit_waiting(
    app: &tauri::AppHandle,
    run_id: &str,
    old_root: &str,
    job_id: &str,
    status: RelayerJobStatus,
) -> Result<(), String> {
    emit_phase(
        app,
        run_id,
        "commit",
        "running",
        format!("Waiting for relayer job {job_id}"),
        None,
        Some(format!("status: {}", status.as_str())),
        Some(old_root),
        None,
        None,
    )
}

pub(super) fn emit_commit_done(
    app: &tauri::AppHandle,
    run_id: &str,
    da_receipt: &str,
    result: &RunSdkActionResult,
) -> Result<(), String> {
    emit_phase(
        app,
        run_id,
        "commit",
        "done",
        format!("Commit complete ({da_receipt})"),
        None,
        Some(result.new_root.clone()),
        Some(&result.old_root),
        Some(&result.new_root),
        result.output_files.first().cloned(),
    )
}
