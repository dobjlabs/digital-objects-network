use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Result};
use craft_sdk::SpendableObjects;
use pod2::middleware::Hash;
use serde::{Deserialize, Serialize};
use txlib::object_nullifier_hash;

use super::{
    engine::{build_relayer_payload, execute_action},
    object_store::{parse_object_file_from_path, write_object_file},
    progress::{
        emit_commit_done, emit_commit_step, emit_generate_proof_done, emit_generate_proof_step,
    },
    relayer_client::{
        submit_proof_to_relayer, wait_for_relayer_confirmation, JobStatus,
        RELAYER_POLL_INTERVAL_MS, RELAYER_POLL_TIMEOUT_SECS,
    },
    runtime::{acquire_run_in_progress_guard, ActionRunGate},
    synchronizer_client::{
        encode_hash_hex, fetch_grounding_witness, wait_for_synchronizer_tx, SynchronizerHead,
        SYNCHRONIZER_POLL_INTERVAL_MS, SYNCHRONIZER_POLL_TIMEOUT_SECS,
    },
};
use crate::{
    error::CommandError,
    objects::{objects_dir, ObjectRecord},
    settings::get_app_settings,
    spec::{self, action_descriptors_by_name},
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSdkActionInput {
    pub action_id: String,
    pub input_object_paths: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSdkActionResult {
    pub ok: bool,
    pub old_root: String,
    pub new_root: String,
    pub output_files: Vec<String>,
    pub nullified_files: Vec<String>,
}

#[derive(Debug)]
struct ResolvedInput {
    file_name: String,
    record: ObjectRecord,
}

fn file_name_from_path(path: &Path) -> Result<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("invalid input path (missing file name): {}", path.display()))
}

fn validate_run_request(
    input: &RunSdkActionInput,
    descriptor: &spec::ActionDescriptor,
) -> Result<()> {
    if descriptor.hidden {
        return Err(anyhow!(
            "action is internal and cannot be run directly: {}",
            input.action_id
        ));
    }

    if input.input_object_paths.len() != descriptor.input_classes.len() {
        return Err(anyhow!(
            "{} expects {} inputs, got {}",
            input.action_id,
            descriptor.input_classes.len(),
            input.input_object_paths.len()
        ));
    }

    let mut seen_paths = HashSet::new();
    for object_path_raw in &input.input_object_paths {
        let object_path = object_path_raw.trim();
        if object_path.is_empty() {
            return Err(anyhow!(
                "each inputObjectPaths entry must be a non-empty path"
            ));
        }
        if !seen_paths.insert(object_path.to_string()) {
            return Err(anyhow!(
                "duplicate input object path is not allowed: {object_path}"
            ));
        }
    }

    Ok(())
}

fn resolve_inputs(
    input: &RunSdkActionInput,
    descriptor: &spec::ActionDescriptor,
) -> Result<Vec<ResolvedInput>> {
    let mut resolved_inputs = Vec::new();

    for (slot, object_path_raw) in input.input_object_paths.iter().enumerate() {
        let expected_class = descriptor.input_classes[slot].as_str();
        let object_path = object_path_raw.trim();
        if object_path.is_empty() {
            return Err(anyhow!("missing objectPath for input slot {}", slot + 1));
        }

        let path_ref = Path::new(object_path);
        let record = parse_object_file_from_path(path_ref)?;
        let file_name = file_name_from_path(path_ref)?;
        if record.is_nullified() {
            return Err(anyhow!("input object is not live: {}", record.id));
        }
        if record.class_name != expected_class {
            return Err(anyhow!(
                "input class mismatch for {}: expected {}, got {}",
                record.id,
                expected_class,
                record.class_name
            ));
        }
        resolved_inputs.push(ResolvedInput { file_name, record });
    }

    Ok(resolved_inputs)
}

struct RelayerSubmitRequest<'a> {
    run_id: &'a str,
    old_root: &'a str,
    relayer_url: &'a str,
    action_id: &'a str,
    payload_bytes: Vec<u8>,
    timeout_secs: u64,
    poll_interval_ms: u64,
}

async fn submit_and_confirm_relayer(
    app: &tauri::AppHandle,
    request: RelayerSubmitRequest<'_>,
) -> Result<()> {
    let RelayerSubmitRequest {
        run_id,
        old_root,
        relayer_url,
        action_id,
        payload_bytes,
        timeout_secs,
        poll_interval_ms,
    } = request;

    emit_commit_step(app, run_id, "Submitting proof to relayer", old_root)?;

    let relayer_url_for_submit = relayer_url.to_string();
    let action_ref = action_id.to_string();
    let submit_job = tauri::async_runtime::spawn_blocking(move || {
        submit_proof_to_relayer(
            &relayer_url_for_submit,
            &payload_bytes,
            Some(format!("app-gui:{action_ref}")),
        )
    })
    .await;

    let submit_response = match submit_job {
        Ok(Ok(response)) => response,
        Ok(Err(err)) => return Err(err),
        Err(err) => return Err(anyhow!("failed while submitting proof to relayer: {err}")),
    };

    if submit_response.status == JobStatus::Failed {
        return Err(anyhow!(
            "relayer rejected job {} immediately",
            submit_response.job_id
        ));
    }

    emit_commit_step(
        app,
        run_id,
        format!("Waiting for relayer job {}", submit_response.job_id).as_str(),
        old_root,
    )?;

    let relayer_url_for_wait = relayer_url.to_string();
    let job_id_for_wait = submit_response.job_id.clone();
    let wait_job = tauri::async_runtime::spawn_blocking(move || {
        wait_for_relayer_confirmation(
            &relayer_url_for_wait,
            &job_id_for_wait,
            timeout_secs,
            poll_interval_ms,
        )
    })
    .await;

    match wait_job {
        Ok(Ok(status)) => status,
        Ok(Err(err)) => return Err(err),
        Err(err) => return Err(anyhow!("failed while polling relayer job status: {err}")),
    };

    Ok(())
}

fn wait_for_synchronizer_commit(
    sync_api_url: &str,
    expected_tx_final: Hash,
    timeout_secs: u64,
    poll_interval_ms: u64,
) -> Result<SynchronizerHead> {
    wait_for_synchronizer_tx(
        sync_api_url,
        expected_tx_final,
        timeout_secs,
        poll_interval_ms,
    )
    .map_err(|err| {
        anyhow!("failed to observe relayed tx in synchronizer after relay confirmation: {err}")
    })
}

struct SavedFiles {
    output_files: Vec<String>,
    nullified_files: Vec<String>,
}

fn save_results(
    objects_dir: &Path,
    descriptor: &spec::ActionDescriptor,
    action_id: &str,
    resolved_inputs: &[ResolvedInput],
    spendable_outputs: &SpendableObjects,
) -> Result<SavedFiles> {
    let mut nullified_files = Vec::new();
    for input in resolved_inputs {
        let input_record = &input.record;
        let spendable = input_record.spendable();
        let nullifier_hash = object_nullifier_hash(&spendable.obj).map_err(|err| {
            anyhow!(
                "failed to compute input nullifier for {}: {err}",
                input_record.id
            )
        })?;
        let input_nullifier = encode_hash_hex(&nullifier_hash);

        let nullified_record = ObjectRecord {
            id: input_record.id.clone(),
            class_name: input_record.class_name.clone(),
            source_action: input_record.source_action.clone(),
            nullifier: Some(input_nullifier),
            pod: input_record.pod.clone(),
            obj: input_record.obj.clone(),
            tx: input_record.tx.clone(),
        };
        write_object_file(&nullified_record, &input.file_name, objects_dir)?;
        nullified_files.push(input.file_name.clone());
    }

    if spendable_outputs.objs.len() != descriptor.output_classes.len() {
        return Err(anyhow!(
            "action {} output mismatch: descriptor expects {}, engine returned {}",
            action_id,
            descriptor.output_classes.len(),
            spendable_outputs.objs.len()
        ));
    }

    let mut output_files = Vec::new();
    for (index, class_name) in descriptor.output_classes.iter().enumerate() {
        let spendable = spendable_outputs.obj(index);
        let object_id = format!("{:#}", spendable.obj.commitment());
        let file_name = format!(
            "{}_{}.dobj",
            class_name.to_ascii_lowercase(),
            object_id.to_ascii_lowercase()
        );

        output_files.push(file_name.clone());
        let live_record = ObjectRecord {
            id: object_id,
            class_name: class_name.clone(),
            source_action: action_id.to_string(),
            nullifier: None,
            pod: spendable.pod,
            obj: spendable.obj,
            tx: spendable.tx,
        };
        write_object_file(&live_record, &file_name, objects_dir)?;
    }

    Ok(SavedFiles {
        output_files,
        nullified_files,
    })
}

fn rollback_results(objects_dir: &Path, resolved_inputs: &[ResolvedInput], saved: &SavedFiles) {
    for input in resolved_inputs {
        let live_record = ObjectRecord {
            id: input.record.id.clone(),
            class_name: input.record.class_name.clone(),
            source_action: input.record.source_action.clone(),
            nullifier: None,
            pod: input.record.pod.clone(),
            obj: input.record.obj.clone(),
            tx: input.record.tx.clone(),
        };
        if let Err(e) = write_object_file(&live_record, &input.file_name, objects_dir) {
            eprintln!("zk-craft: rollback failed for {}: {e}", input.file_name);
        }
    }
    for file_name in &saved.output_files {
        let path = objects_dir.join(file_name);
        match std::fs::remove_file(&path) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                eprintln!("zk-craft: rollback failed to remove {file_name}: {e}");
            }
        }
    }
}

#[tauri::command]
pub async fn run_sdk_action(
    app: tauri::AppHandle,
    runtime: tauri::State<'_, std::sync::Arc<ActionRunGate>>,
    input: RunSdkActionInput,
) -> Result<RunSdkActionResult, CommandError> {
    run_sdk_action_core(app, &runtime, input).await
}

/// Core action pipeline shared by the Tauri command and MCP server.
pub(crate) async fn run_sdk_action_core(
    app: tauri::AppHandle,
    runtime: &ActionRunGate,
    input: RunSdkActionInput,
) -> Result<RunSdkActionResult, CommandError> {
    let objects_dir: PathBuf = objects_dir(&app)?;
    let descriptors = action_descriptors_by_name();
    let descriptor = descriptors
        .get(&input.action_id)
        .cloned()
        .ok_or_else(|| anyhow!("unknown action: {}", input.action_id))?;

    validate_run_request(&input, &descriptor)?;

    let app_settings = get_app_settings(app.clone())?;
    let _run_guard = acquire_run_in_progress_guard(runtime)?;

    let action_id = input.action_id.clone();
    emit_generate_proof_step(&app, &action_id, "Verifying Inputs")?;

    let resolved_inputs = resolve_inputs(&input, &descriptor)?;
    let source_tx_hashes = resolved_inputs
        .iter()
        .map(|input| input.record.spendable().tx.dict().commitment())
        .collect::<Vec<_>>();
    let grounding_witness =
        fetch_grounding_witness(&app_settings.synchronizer_api_url, &source_tx_hashes)?;
    let old_root_hash = grounding_witness.state_root.hash();
    let old_root = encode_hash_hex(&old_root_hash);

    emit_generate_proof_step(&app, &action_id, "Generating proof")?;

    let execution_inputs = resolved_inputs
        .iter()
        .map(|input| input.record.spendable())
        .collect::<Vec<_>>();

    let action_id_for_exec = action_id.clone();
    let spendable_outputs = match tauri::async_runtime::spawn_blocking(move || {
        execute_action(action_id_for_exec, grounding_witness, execution_inputs)
    })
    .await
    {
        Ok(result) => result,
        Err(err) => Err(anyhow!("failed while executing action: {err}")),
    }?;

    emit_generate_proof_done(&app, &action_id)?;

    emit_commit_step(&app, &action_id, "Shrinking proof", &old_root)?;
    let payload_bytes = build_relayer_payload(&old_root_hash, &spendable_outputs)?;
    let expected_tx_final = spendable_outputs.tx.dict().commitment();

    emit_commit_step(&app, &action_id, "Creating files", &old_root)?;
    let saved = save_results(
        &objects_dir,
        &descriptor,
        &action_id,
        &resolved_inputs,
        &spendable_outputs,
    )?;

    let commit_result: Result<SynchronizerHead> = async {
        submit_and_confirm_relayer(
            &app,
            RelayerSubmitRequest {
                run_id: &action_id,
                old_root: &old_root,
                relayer_url: &app_settings.relayer_api_url,
                action_id: &action_id,
                payload_bytes,
                timeout_secs: RELAYER_POLL_TIMEOUT_SECS,
                poll_interval_ms: RELAYER_POLL_INTERVAL_MS,
            },
        )
        .await?;

        emit_commit_step(
            &app,
            &action_id,
            "Waiting for synchronizer to observe commit",
            &old_root,
        )?;
        wait_for_synchronizer_commit(
            &app_settings.synchronizer_api_url,
            expected_tx_final,
            SYNCHRONIZER_POLL_TIMEOUT_SECS,
            SYNCHRONIZER_POLL_INTERVAL_MS,
        )
    }
    .await;

    match commit_result {
        Ok(sync_state_after) => {
            let new_root = encode_hash_hex(&sync_state_after.current_gsr);
            let result = RunSdkActionResult {
                ok: true,
                old_root: old_root.clone(),
                new_root,
                output_files: saved.output_files,
                nullified_files: saved.nullified_files,
            };
            emit_commit_done(&app, &action_id, &result)?;
            Ok(result)
        }
        Err(err) => {
            rollback_results(&objects_dir, &resolved_inputs, &saved);
            Err(err.into())
        }
    }
}
