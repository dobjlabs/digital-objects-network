use crate::types::{CreateDobjInput, CreateDobjProgress, CreateDobjResult};
use std::hint::black_box;
use std::time::{Duration, Instant};
use tauri::Emitter;

fn fake_hash(seed: &str) -> String {
    let mut bytes = [0u8; 8];
    for (idx, b) in seed.bytes().enumerate() {
        bytes[idx % 8] = bytes[idx % 8].wrapping_add(b);
    }
    format!(
        "0x{:02x}{:02x}...{:02x}{:02x}",
        bytes[0], bytes[1], bytes[6], bytes[7]
    )
}

fn burn_cpu_for(duration: Duration) {
    let start = Instant::now();
    let mut acc: u64 = 0x9e37_79b9_7f4a_7c15;

    // Intentional tight loop to generate measurable app CPU load for mock runs.
    while start.elapsed() < duration {
        for i in 0..250_000_u64 {
            acc = acc.rotate_left(13) ^ (i.wrapping_mul(0xbf58_476d_1ce4_e5b9));
            acc = acc.wrapping_mul(0x94d0_49bb_1331_11eb);
        }
    }

    let _ = black_box(acc);
}

fn emit_progress(app: &tauri::AppHandle, payload: &CreateDobjProgress) -> Result<(), String> {
    app.emit("create-dobj-progress", payload)
        .map_err(|err| format!("failed to emit create_dobj progress: {err}"))
}

#[tauri::command]
pub async fn create_dobj(
    app: tauri::AppHandle,
    input: CreateDobjInput,
) -> Result<CreateDobjResult, String> {
    let verify_targets = if input.input_files.is_empty() {
        vec!["(no inputs)".to_string()]
    } else {
        input.input_files.clone()
    };

    emit_progress(
        &app,
        &CreateDobjProgress {
            dobj_id: input.dobj_id.clone(),
            phase: "hash".to_string(),
            status: "running".to_string(),
            message: "Hashing".to_string(),
            verify_index: None,
            detail: Some("20-40s".to_string()),
            old_root: None,
            new_root: None,
            output_file: None,
        },
    )?;
    tauri::async_runtime::spawn_blocking(|| burn_cpu_for(Duration::from_secs(3)))
        .await
        .map_err(|err| format!("failed during hash phase: {err}"))?;
    emit_progress(
        &app,
        &CreateDobjProgress {
            dobj_id: input.dobj_id.clone(),
            phase: "hash".to_string(),
            status: "done".to_string(),
            message: "Hash complete".to_string(),
            verify_index: None,
            detail: Some("20-40s".to_string()),
            old_root: None,
            new_root: None,
            output_file: None,
        },
    )?;

    for (index, target) in verify_targets.iter().enumerate() {
        emit_progress(
            &app,
            &CreateDobjProgress {
                dobj_id: input.dobj_id.clone(),
                phase: "verify".to_string(),
                status: "running".to_string(),
                message: format!("Verifying {target}"),
                verify_index: Some(index),
                detail: Some(target.clone()),
                old_root: None,
                new_root: None,
                output_file: None,
            },
        )?;
        tauri::async_runtime::spawn_blocking(|| burn_cpu_for(Duration::from_secs(2)))
            .await
            .map_err(|err| format!("failed during verify phase: {err}"))?;
        emit_progress(
            &app,
            &CreateDobjProgress {
                dobj_id: input.dobj_id.clone(),
                phase: "verify".to_string(),
                status: "done".to_string(),
                message: format!("Verified {target}"),
                verify_index: Some(index),
                detail: Some(target.clone()),
                old_root: None,
                new_root: None,
                output_file: None,
            },
        )?;
    }

    let seed = format!("{}:{}", input.dobj_id, input.input_files.join(","));
    let old_root = fake_hash(&format!("{seed}:old"));
    let new_root = fake_hash(&format!("{seed}:new"));
    let output_file = format!("{}.dobj", input.dobj_id);

    emit_progress(
        &app,
        &CreateDobjProgress {
            dobj_id: input.dobj_id.clone(),
            phase: "nullify".to_string(),
            status: "running".to_string(),
            message: format!("Nullifying {old_root}"),
            verify_index: None,
            detail: Some(old_root.clone()),
            old_root: Some(old_root.clone()),
            new_root: None,
            output_file: None,
        },
    )?;
    tauri::async_runtime::spawn_blocking(|| burn_cpu_for(Duration::from_secs(1)))
        .await
        .map_err(|err| format!("failed during nullify phase: {err}"))?;
    emit_progress(
        &app,
        &CreateDobjProgress {
            dobj_id: input.dobj_id.clone(),
            phase: "nullify".to_string(),
            status: "done".to_string(),
            message: "Nullify complete".to_string(),
            verify_index: None,
            detail: Some(old_root.clone()),
            old_root: Some(old_root.clone()),
            new_root: None,
            output_file: None,
        },
    )?;

    emit_progress(
        &app,
        &CreateDobjProgress {
            dobj_id: input.dobj_id.clone(),
            phase: "commit".to_string(),
            status: "running".to_string(),
            message: format!("Committing {new_root}"),
            verify_index: None,
            detail: Some(new_root.clone()),
            old_root: Some(old_root.clone()),
            new_root: Some(new_root.clone()),
            output_file: Some(output_file.clone()),
        },
    )?;
    tauri::async_runtime::spawn_blocking(|| burn_cpu_for(Duration::from_secs(2)))
        .await
        .map_err(|err| format!("failed during commit phase: {err}"))?;
    emit_progress(
        &app,
        &CreateDobjProgress {
            dobj_id: input.dobj_id.clone(),
            phase: "commit".to_string(),
            status: "done".to_string(),
            message: format!("Created {output_file}"),
            verify_index: None,
            detail: Some(new_root.clone()),
            old_root: Some(old_root.clone()),
            new_root: Some(new_root.clone()),
            output_file: Some(output_file.clone()),
        },
    )?;

    Ok(CreateDobjResult {
        ok: true,
        old_root,
        new_root,
        output_file,
    })
}
