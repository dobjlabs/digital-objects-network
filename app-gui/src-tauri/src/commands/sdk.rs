use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use common::{
    payload::{Payload, PayloadProof},
    shrink::{ShrunkMainPodSetup, shrink_compress_pod},
};
use craftlib::{
    scenario::test_sdk,
    sdk::{Helper, SpendableObject, SpendableObjects},
};
use hex::{FromHex, ToHex};
use pod2::middleware::{Hash, Params};
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager};
use txlib::StateRoot;

use crate::{
    state::{CraftRuntime, CraftRuntimeInner, RuntimeObjectRecord, RuntimeValidity},
    types::{
        ClassMetaDto, RunSdkActionProgress, InventoryItemDto, ItemStatDto, LoadGuiBootstrapResult,
        MethodArgDto, ObjectMethodDto, RecipeDto, RunSdkActionInput, RunSdkActionResult,
        SourceActionMetaDto,
    },
};

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedStateRoot {
    transactions: serde_json::Value,
    nullifiers: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedSpendableObject {
    pod: serde_json::Value,
    obj: serde_json::Value,
    tx_live: serde_json::Value,
    tx_nullifiers: serde_json::Value,
    tx_state_root: PersistedStateRoot,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedObjectRecord {
    id: String,
    file_name: String,
    class_name: String,
    source_action: Option<String>,
    validity: String,
    state_hash: String,
    nullifier: Option<String>,
    stats: Vec<(String, String)>,
    spendable: Option<PersistedSpendableObject>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SynchronizerStateResponse {
    transactions: Vec<String>,
    nullifiers: Vec<String>,
    current_gsr: Option<String>,
}

struct SynchronizerState {
    state_root: StateRoot,
    current_gsr: Hash,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DaPostRequest {
    action_id: String,
    payload_hex: String,
    tx_final: String,
    state_root_hash: String,
    nullifiers: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DaPostResponse {
    #[serde(alias = "txHash", alias = "tx_hash", alias = "id")]
    tx_hash: Option<String>,
}

fn short_hash(seed: &str) -> String {
    let mut bytes = [0u8; 8];
    for (idx, b) in seed.bytes().enumerate() {
        bytes[idx % 8] = bytes[idx % 8].wrapping_add(b);
    }
    format!(
        "0x{:02x}{:02x}...{:02x}{:02x}",
        bytes[0], bytes[1], bytes[6], bytes[7]
    )
}

fn resolve_objects_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let home = app
        .path()
        .home_dir()
        .map_err(|err| format!("failed to resolve home directory: {err}"))?;
    Ok(home.join(".objects"))
}

fn synchronizer_api_url() -> String {
    std::env::var("ZKCRAFT_SYNCHRONIZER_API_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:3000".to_string())
}

fn da_post_url() -> Result<String, String> {
    let url = std::env::var("ZKCRAFT_DA_POST_URL")
        .map_err(|_| "ZKCRAFT_DA_POST_URL is required to commit actions to DA".to_string())?;
    if url.trim().is_empty() {
        return Err("ZKCRAFT_DA_POST_URL is empty".to_string());
    }
    Ok(url)
}

fn parse_hash_hex(value: &str) -> Result<Hash, String> {
    let trimmed = value.trim().strip_prefix("0x").unwrap_or(value.trim());
    Hash::from_hex(trimmed).map_err(|err| format!("invalid hash {value}: {err}"))
}

fn encode_hash_hex(hash: &Hash) -> String {
    format!("0x{}", hash.encode_hex::<String>())
}

fn fetch_synchronizer_state(sync_api_url: &str) -> Result<SynchronizerState, String> {
    let endpoint = format!("{}/state", sync_api_url.trim_end_matches('/'));
    let response = reqwest::blocking::get(&endpoint)
        .map_err(|err| format!("failed to query synchronizer at {endpoint}: {err}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "synchronizer request failed: {} {}",
            response.status().as_u16(),
            response.status()
        ));
    }
    let payload: SynchronizerStateResponse = response
        .json()
        .map_err(|err| format!("failed to decode synchronizer response: {err}"))?;

    let transactions = payload
        .transactions
        .iter()
        .map(|entry| parse_hash_hex(entry))
        .collect::<Result<HashSet<_>, String>>()?;
    let nullifiers = payload
        .nullifiers
        .iter()
        .map(|entry| parse_hash_hex(entry))
        .collect::<Result<HashSet<_>, String>>()?;

    let state_root = StateRoot::new(0, &transactions, &nullifiers, &[]);
    let derived_gsr = state_root.hash();
    let current_gsr = if let Some(gsr) = payload.current_gsr.as_deref() {
        let remote_gsr = parse_hash_hex(gsr)?;
        if remote_gsr != derived_gsr {
            eprintln!(
                "zk-craft: synchronizer current_gsr mismatch (derived={}, remote={})",
                encode_hash_hex(&derived_gsr),
                encode_hash_hex(&remote_gsr)
            );
        }
        remote_gsr
    } else {
        derived_gsr
    };

    Ok(SynchronizerState {
        state_root,
        current_gsr,
    })
}

fn empty_state_root() -> StateRoot {
    let empty = HashSet::new();
    StateRoot::new(0, &empty, &empty, &[])
}

fn clone_spendable(spendable: &SpendableObject) -> SpendableObject {
    SpendableObject {
        pod: spendable.pod.clone(),
        obj: spendable.obj.clone(),
        tx: spendable.tx.clone(),
    }
}

fn normalize_component_name(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

fn format_output_file_name(class_name: &str, index: u64) -> String {
    format!("{}_{index}.dobj", normalize_component_name(class_name))
}

fn value_string(raw: String) -> String {
    let trimmed = raw.trim();
    if trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2 {
        trimmed[1..trimmed.len() - 1].to_string()
    } else {
        trimmed.to_string()
    }
}

fn stats_from_object(spendable: &SpendableObject) -> Vec<(String, String)> {
    let mut stats = Vec::new();
    for (key, value) in spendable.obj.kvs() {
        let name = key.name();
        if matches!(name, "key" | "work" | "blueprint") {
            continue;
        }
        stats.push((name.to_string(), value_string(format!("{value}"))));
    }
    stats.sort_by(|a, b| a.0.cmp(&b.0));
    stats
}

fn action_descriptors_by_name() -> HashMap<String, craftlib::scenario::test_sdk::ActionDescriptor> {
    let mut out = HashMap::new();
    for descriptor in test_sdk::action_descriptors() {
        out.insert(descriptor.name.clone(), descriptor);
    }
    out
}

fn build_action_catalog() -> Vec<RecipeDto> {
    test_sdk::action_descriptors()
        .into_iter()
        .filter(|descriptor| !descriptor.hidden)
        .map(|descriptor| RecipeDto {
            id: descriptor.name.clone(),
            group: String::new(),
            name: descriptor.name.clone(),
            emoji: descriptor.ui.emoji.to_string(),
            hash: short_hash(&descriptor.name),
            verb: descriptor.name.clone(),
            desc: descriptor.ui.description.to_string(),
            cpu: descriptor.ui.cpu_cost.to_string(),
            reads_block: descriptor.ui.reads_block,
            args: descriptor
                .input_classes
                .into_iter()
                .map(|class_name| MethodArgDto {
                    kind: "class".to_string(),
                    label: class_name.clone(),
                    class_hash: short_hash(&class_name),
                })
                .collect(),
            unlocked: true,
        })
        .collect()
}

fn to_inventory_item(record: &RuntimeObjectRecord) -> InventoryItemDto {
    let class_ui = test_sdk::class_ui_meta(&record.class_name);
    InventoryItemDto {
        id: record.id.clone(),
        file_name: record.file_name.clone(),
        emoji: class_ui.emoji.to_string(),
        validity: match record.validity {
            RuntimeValidity::Live => "live".to_string(),
            RuntimeValidity::Nullified => "nullified".to_string(),
        },
        state_root: record.state_hash.clone(),
        nullifier: record.nullifier.clone(),
        class_meta: ClassMetaDto {
            name: record.class_name.clone(),
            hash: short_hash(&record.class_name),
        },
        source_action: record.source_action.as_ref().map(|name| SourceActionMetaDto {
            name: name.clone(),
            hash: short_hash(name),
        }),
        description: Some(class_ui.description.to_string()),
        methods: Vec::<ObjectMethodDto>::new(),
        stats: record
            .stats
            .iter()
            .map(|(key, value)| ItemStatDto {
                key: key.clone(),
                value: value.clone(),
                tone: None,
            })
            .collect(),
    }
}

fn persist_state_root(state_root: &StateRoot) -> Result<PersistedStateRoot, String> {
    Ok(PersistedStateRoot {
        transactions: serde_json::to_value(&state_root.transactions)
            .map_err(|err| format!("failed to serialize state_root.transactions: {err}"))?,
        nullifiers: serde_json::to_value(&state_root.nullifiers)
            .map_err(|err| format!("failed to serialize state_root.nullifiers: {err}"))?,
    })
}

fn restore_state_root(data: PersistedStateRoot) -> Result<StateRoot, String> {
    Ok(StateRoot {
        transactions: serde_json::from_value(data.transactions)
            .map_err(|err| format!("failed to deserialize state_root.transactions: {err}"))?,
        nullifiers: serde_json::from_value(data.nullifiers)
            .map_err(|err| format!("failed to deserialize state_root.nullifiers: {err}"))?,
    })
}

fn persist_spendable(spendable: &SpendableObject) -> Result<PersistedSpendableObject, String> {
    Ok(PersistedSpendableObject {
        pod: serde_json::to_value(&spendable.pod)
            .map_err(|err| format!("failed to serialize spendable.pod: {err}"))?,
        obj: serde_json::to_value(&spendable.obj)
            .map_err(|err| format!("failed to serialize spendable.obj: {err}"))?,
        tx_live: serde_json::to_value(&spendable.tx.live)
            .map_err(|err| format!("failed to serialize spendable.tx.live: {err}"))?,
        tx_nullifiers: serde_json::to_value(&spendable.tx.nullifiers)
            .map_err(|err| format!("failed to serialize spendable.tx.nullifiers: {err}"))?,
        tx_state_root: persist_state_root(spendable.tx.state_root.as_ref())?,
    })
}

fn restore_spendable(data: PersistedSpendableObject) -> Result<SpendableObject, String> {
    let state_root = restore_state_root(data.tx_state_root)?;
    let tx = txlib::Tx {
        live: serde_json::from_value(data.tx_live)
            .map_err(|err| format!("failed to deserialize spendable.tx.live: {err}"))?,
        nullifiers: serde_json::from_value(data.tx_nullifiers)
            .map_err(|err| format!("failed to deserialize spendable.tx.nullifiers: {err}"))?,
        state_root: Arc::new(state_root),
    };
    Ok(SpendableObject {
        pod: serde_json::from_value(data.pod)
            .map_err(|err| format!("failed to deserialize spendable.pod: {err}"))?,
        obj: serde_json::from_value(data.obj)
            .map_err(|err| format!("failed to deserialize spendable.obj: {err}"))?,
        tx,
    })
}

fn validity_from_str(raw: &str, context: &str) -> Result<RuntimeValidity, String> {
    match raw {
        "live" => Ok(RuntimeValidity::Live),
        "nullified" => Ok(RuntimeValidity::Nullified),
        other => Err(format!("invalid object validity in {context}: {other}")),
    }
}

fn restore_object_record(
    record: PersistedObjectRecord,
    file_name_override: Option<&str>,
) -> Result<RuntimeObjectRecord, String> {
    Ok(RuntimeObjectRecord {
        id: record.id,
        file_name: file_name_override.unwrap_or(&record.file_name).to_string(),
        class_name: record.class_name,
        source_action: record.source_action,
        validity: validity_from_str(&record.validity, "object file")?,
        state_hash: record.state_hash,
        nullifier: record.nullifier,
        stats: record.stats,
        spendable: record.spendable.map(restore_spendable).transpose()?,
    })
}

fn parse_object_file(contents: &str, file_name: &str) -> Result<RuntimeObjectRecord, String> {
    let record = serde_json::from_str::<PersistedObjectRecord>(contents)
        .map_err(|err| format!("failed to parse {file_name} as object file: {err}"))?;
    restore_object_record(record, Some(file_name))
}

fn persist_object_record(record: &RuntimeObjectRecord) -> Result<PersistedObjectRecord, String> {
    Ok(PersistedObjectRecord {
        id: record.id.clone(),
        file_name: record.file_name.clone(),
        class_name: record.class_name.clone(),
        source_action: record.source_action.clone(),
        validity: match record.validity {
            RuntimeValidity::Live => "live".to_string(),
            RuntimeValidity::Nullified => "nullified".to_string(),
        },
        state_hash: record.state_hash.clone(),
        nullifier: record.nullifier.clone(),
        stats: record.stats.clone(),
        spendable: record.spendable.as_ref().map(persist_spendable).transpose()?,
    })
}

fn sync_object_files(inner: &CraftRuntimeInner, objects_dir: &Path) -> Result<(), String> {
    fs::create_dir_all(objects_dir)
        .map_err(|err| format!("failed to create objects directory: {err}"))?;

    for record in &inner.objects {
        let persisted = persist_object_record(record)?;
        let serialized = serde_json::to_string_pretty(&persisted)
            .map_err(|err| format!("failed to serialize object file {}: {err}", record.file_name))?;
        fs::write(objects_dir.join(&record.file_name), serialized)
            .map_err(|err| format!("failed to write object file {}: {err}", record.file_name))?;
    }

    Ok(())
}

fn load_object_files(objects_dir: &Path) -> Result<Vec<RuntimeObjectRecord>, String> {
    let mut objects = Vec::new();
    for entry in
        fs::read_dir(objects_dir).map_err(|err| format!("failed to read objects directory: {err}"))?
    {
        let entry = entry.map_err(|err| format!("failed to read objects entry: {err}"))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let is_dobj = path.extension().and_then(|ext| ext.to_str()) == Some("dobj");
        if !is_dobj {
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };

        let contents = match fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(err) => {
                eprintln!("zk-craft: failed to read {file_name}, skipping: {err}");
                continue;
            }
        };

        match parse_object_file(&contents, file_name) {
            Ok(record) => objects.push(record),
            Err(err) => eprintln!("zk-craft: failed to parse {file_name}, skipping: {err}"),
        }
    }

    objects.sort_by(|a, b| a.file_name.cmp(&b.file_name));
    Ok(objects)
}

fn next_object_index_from_records(objects: &[RuntimeObjectRecord]) -> u64 {
    let max_index = objects
        .iter()
        .filter_map(|record| {
            record
                .id
                .strip_prefix("obj-")
                .and_then(|suffix| suffix.parse::<u64>().ok())
        })
        .max()
        .unwrap_or(0);
    max_index + 1
}

fn ensure_runtime_loaded(inner: &mut CraftRuntimeInner, objects_dir: &Path) -> Result<(), String> {
    if inner.loaded {
        return Ok(());
    }
    fs::create_dir_all(objects_dir)
        .map_err(|err| format!("failed to create objects directory: {err}"))?;
    inner.objects = load_object_files(objects_dir)?;
    inner.next_object_index = next_object_index_from_records(&inner.objects);
    inner.state_root = empty_state_root();
    inner.loaded = true;
    Ok(())
}

fn emit_progress(app: &tauri::AppHandle, payload: &RunSdkActionProgress) -> Result<(), String> {
    app.emit("run-sdk-action-progress", payload)
        .map_err(|err| format!("failed to emit run progress: {err}"))
}

fn clear_run_in_progress(runtime: &tauri::State<'_, CraftRuntime>) {
    if let Ok(mut inner) = runtime.inner.lock() {
        inner.run_in_progress = false;
    }
}

fn lock_runtime<'a>(
    runtime: &'a tauri::State<'_, CraftRuntime>,
) -> std::sync::MutexGuard<'a, CraftRuntimeInner> {
    match runtime.inner.lock() {
        Ok(inner) => inner,
        Err(poisoned) => {
            eprintln!("zk-craft: runtime lock poisoned, recovering state");
            poisoned.into_inner()
        }
    }
}

#[tauri::command]
pub async fn load_gui_bootstrap(
    app: tauri::AppHandle,
    runtime: tauri::State<'_, CraftRuntime>,
) -> Result<LoadGuiBootstrapResult, String> {
    let objects_dir = resolve_objects_dir(&app)?;
    let actions = build_action_catalog();
    let sync_state = fetch_synchronizer_state(&synchronizer_api_url());
    let mut inner = lock_runtime(&runtime);
    if let Err(err) = ensure_runtime_loaded(&mut inner, &objects_dir) {
        eprintln!("zk-craft: bootstrap runtime failed, resetting state: {err}");
        inner.next_object_index = 1;
        inner.state_root = empty_state_root();
        inner.objects.clear();
        inner.loaded = true;
        let _ = sync_object_files(&inner, &objects_dir);
    }
    match sync_state {
        Ok(state) => inner.state_root = state.state_root,
        Err(err) => eprintln!("zk-craft: synchronizer unavailable during bootstrap: {err}"),
    }

    Ok(LoadGuiBootstrapResult {
        objects: inner.objects.iter().map(to_inventory_item).collect(),
        actions,
    })
}

fn execute_action(
    action_id: String,
    state_root: StateRoot,
    inputs: Vec<SpendableObject>,
) -> Result<SpendableObjects, String> {
    let helper = Helper::new(test_sdk::dependencies(), test_sdk::actions());
    let builder = helper.builder(true, Arc::new(state_root));
    Ok(builder.action(&action_id, inputs))
}

fn build_da_payload(
    old_state_root_hash: &Hash,
    action_output: &SpendableObjects,
) -> Result<(Vec<u8>, Hash, Vec<Hash>), String> {
    let params = Params::default();
    let shrunk_main_pod = ShrunkMainPodSetup::new(&params)
        .build()
        .map_err(|err| format!("failed to build shrunk proof circuit: {err}"))?;
    let compressed = shrink_compress_pod(&shrunk_main_pod, action_output.tx_pod.clone())
        .map_err(|err| format!("failed to shrink/compress tx proof: {err}"))?;

    let tx_final = action_output.tx.dict().commitment();
    let nullifiers = action_output
        .tx
        .nullifiers
        .set()
        .iter()
        .map(|entry| Hash(entry.raw().0))
        .collect::<Vec<_>>();
    let payload = Payload {
        proof: PayloadProof::Plonky2(Box::new(compressed)),
        tx_final,
        state_root_hash: *old_state_root_hash,
        nullifiers: nullifiers.clone(),
    };

    let payload_bytes = payload.to_bytes();
    Ok((payload_bytes, tx_final, nullifiers))
}

fn post_da_payload(
    da_url: &str,
    action_id: &str,
    payload_bytes: &[u8],
    tx_final: &Hash,
    state_root_hash: &Hash,
    nullifiers: &[Hash],
) -> Result<String, String> {
    let request = DaPostRequest {
        action_id: action_id.to_string(),
        payload_hex: format!("0x{}", hex::encode(payload_bytes)),
        tx_final: encode_hash_hex(tx_final),
        state_root_hash: encode_hash_hex(state_root_hash),
        nullifiers: nullifiers.iter().map(encode_hash_hex).collect(),
    };

    let client = reqwest::blocking::Client::new();
    let response = client
        .post(da_url)
        .json(&request)
        .send()
        .map_err(|err| format!("failed to post payload to DA at {da_url}: {err}"))?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().unwrap_or_default();
        return Err(format!(
            "DA post failed with {} {}: {}",
            status.as_u16(),
            status,
            body
        ));
    }
    let parsed: Result<DaPostResponse, _> = response.json();
    match parsed {
        Ok(value) => Ok(value.tx_hash.unwrap_or_else(|| "posted".to_string())),
        Err(_) => Ok("posted".to_string()),
    }
}

#[tauri::command]
pub async fn run_sdk_action(
    app: tauri::AppHandle,
    runtime: tauri::State<'_, CraftRuntime>,
    input: RunSdkActionInput,
) -> Result<RunSdkActionResult, String> {
    let objects_dir = resolve_objects_dir(&app)?;
    let descriptors = action_descriptors_by_name();
    let descriptor = descriptors
        .get(&input.action_id)
        .ok_or_else(|| format!("unknown action: {}", input.action_id))?;
    if descriptor.hidden {
        return Err(format!("action is internal and cannot be run directly: {}", input.action_id));
    }

    if input.input_object_ids.len() != descriptor.input_classes.len() {
        return Err(format!(
            "{} expects {} inputs, got {}",
            input.action_id,
            descriptor.input_classes.len(),
            input.input_object_ids.len()
        ));
    }

    let mut seen = HashSet::new();
    for object_id in &input.input_object_ids {
        if !seen.insert(object_id) {
            return Err("duplicate input object IDs are not allowed".to_string());
        }
    }

    let sync_api_url = synchronizer_api_url();
    let sync_state = fetch_synchronizer_state(&sync_api_url)?;
    let state_root_for_run = sync_state.state_root.clone();
    let old_root_hash = sync_state.current_gsr;
    let old_root = short_hash(&format!("{:#}", old_root_hash));
    let da_url = da_post_url()?;

    let (input_spendables, verify_targets);
    {
        let mut inner = lock_runtime(&runtime);
        ensure_runtime_loaded(&mut inner, &objects_dir)?;

        if inner.run_in_progress {
            return Err("another action run is already in progress".to_string());
        }
        inner.state_root = state_root_for_run.clone();

        let mut collected_spendables = Vec::new();
        let mut collected_targets = Vec::new();

        for (slot, object_id) in input.input_object_ids.iter().enumerate() {
            let expected_class = descriptor.input_classes[slot].as_str();
            let record = inner
                .objects
                .iter()
                .find(|record| &record.id == object_id)
                .ok_or_else(|| format!("input object not found: {object_id}"))?;

            if record.validity != RuntimeValidity::Live {
                return Err(format!("input object is not live: {object_id}"));
            }
            if record.class_name != expected_class {
                return Err(format!(
                    "input class mismatch for {}: expected {}, got {}",
                    object_id, expected_class, record.class_name
                ));
            }

            let spendable = record
                .spendable
                .as_ref()
                .ok_or_else(|| format!("input object missing spendable witness: {object_id}"))?;
            collected_spendables.push(clone_spendable(spendable));
            collected_targets.push(record.file_name.clone());
        }

        inner.run_in_progress = true;
        input_spendables = collected_spendables;
        verify_targets = collected_targets;
    }

    let run_id = input.action_id.clone();
    if let Err(err) = emit_progress(
        &app,
        &RunSdkActionProgress {
            run_id: run_id.clone(),
            phase: "hash".to_string(),
            status: "running".to_string(),
            message: format!("Running {}", input.action_id),
            verify_index: None,
            detail: Some(descriptor.ui.cpu_cost.to_string()),
            old_root: None,
            new_root: None,
            output_file: None,
        },
    ) {
        clear_run_in_progress(&runtime);
        return Err(err);
    }

    let action_id = input.action_id.clone();
    let execution = match tauri::async_runtime::spawn_blocking(move || {
        execute_action(action_id, state_root_for_run, input_spendables)
    })
    .await
    {
        Ok(value) => value,
        Err(err) => {
            clear_run_in_progress(&runtime);
            return Err(format!("failed while executing action: {err}"));
        }
    };

    let spendable_outputs = match execution {
        Ok(output) => output,
        Err(err) => {
            let mut inner = lock_runtime(&runtime);
            inner.run_in_progress = false;
            return Err(err);
        }
    };

    if let Err(err) = emit_progress(
        &app,
        &RunSdkActionProgress {
            run_id: run_id.clone(),
            phase: "hash".to_string(),
            status: "done".to_string(),
            message: "Proof generation complete".to_string(),
            verify_index: None,
            detail: Some(descriptor.ui.cpu_cost.to_string()),
            old_root: None,
            new_root: None,
            output_file: None,
        },
    ) {
        clear_run_in_progress(&runtime);
        return Err(err);
    }

    if verify_targets.is_empty() {
        let placeholder = "(no inputs)".to_string();
        if let Err(err) = emit_progress(
            &app,
            &RunSdkActionProgress {
                run_id: run_id.clone(),
                phase: "verify".to_string(),
                status: "running".to_string(),
                message: format!("Verifying {placeholder}"),
                verify_index: Some(0),
                detail: Some(placeholder.clone()),
                old_root: None,
                new_root: None,
                output_file: None,
            },
        ) {
            clear_run_in_progress(&runtime);
            return Err(err);
        }
        if let Err(err) = emit_progress(
            &app,
            &RunSdkActionProgress {
                run_id: run_id.clone(),
                phase: "verify".to_string(),
                status: "done".to_string(),
                message: format!("Verified {placeholder}"),
                verify_index: Some(0),
                detail: Some(placeholder),
                old_root: None,
                new_root: None,
                output_file: None,
            },
        ) {
            clear_run_in_progress(&runtime);
            return Err(err);
        }
    } else {
        for (index, target) in verify_targets.iter().enumerate() {
            if let Err(err) = emit_progress(
                &app,
                &RunSdkActionProgress {
                    run_id: run_id.clone(),
                    phase: "verify".to_string(),
                    status: "running".to_string(),
                    message: format!("Verifying {target}"),
                    verify_index: Some(index),
                    detail: Some(target.clone()),
                    old_root: None,
                    new_root: None,
                    output_file: None,
                },
            ) {
                clear_run_in_progress(&runtime);
                return Err(err);
            }
            if let Err(err) = emit_progress(
                &app,
                &RunSdkActionProgress {
                    run_id: run_id.clone(),
                    phase: "verify".to_string(),
                    status: "done".to_string(),
                    message: format!("Verified {target}"),
                    verify_index: Some(index),
                    detail: Some(target.clone()),
                    old_root: None,
                    new_root: None,
                    output_file: None,
                },
            ) {
                clear_run_in_progress(&runtime);
                return Err(err);
            }
        }
    }

    if let Err(err) = emit_progress(
        &app,
        &RunSdkActionProgress {
            run_id: run_id.clone(),
            phase: "nullify".to_string(),
            status: "running".to_string(),
            message: format!("Nullifying {old_root}"),
            verify_index: None,
            detail: Some(old_root.clone()),
            old_root: Some(old_root.clone()),
            new_root: None,
            output_file: None,
        },
    ) {
        clear_run_in_progress(&runtime);
        return Err(err);
    }

    emit_progress(
        &app,
        &RunSdkActionProgress {
            run_id: run_id.clone(),
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

    let (payload_bytes, tx_final, payload_nullifiers) =
        match build_da_payload(&old_root_hash, &spendable_outputs) {
            Ok(payload) => payload,
            Err(err) => {
                clear_run_in_progress(&runtime);
                return Err(err);
            }
        };

    emit_progress(
        &app,
        &RunSdkActionProgress {
            run_id: run_id.clone(),
            phase: "commit".to_string(),
            status: "running".to_string(),
            message: "Posting payload to Ethereum DA".to_string(),
            verify_index: None,
            detail: Some("posting payload".to_string()),
            old_root: Some(old_root.clone()),
            new_root: None,
            output_file: None,
        },
    )?;

    let da_url_for_post = da_url.clone();
    let action_id_for_post = input.action_id.clone();
    let da_post = tauri::async_runtime::spawn_blocking(move || {
        post_da_payload(
            &da_url_for_post,
            &action_id_for_post,
            &payload_bytes,
            &tx_final,
            &old_root_hash,
            &payload_nullifiers,
        )
    })
    .await;
    let da_receipt = match da_post {
        Ok(Ok(receipt)) => receipt,
        Ok(Err(err)) => {
            clear_run_in_progress(&runtime);
            return Err(err);
        }
        Err(err) => {
            clear_run_in_progress(&runtime);
            return Err(format!("failed while posting payload to DA: {err}"));
        }
    };

    let sync_state_after = match fetch_synchronizer_state(&sync_api_url) {
        Ok(state) => state,
        Err(err) => {
            clear_run_in_progress(&runtime);
            return Err(format!("failed to refresh state root from synchronizer after DA post: {err}"));
        }
    };
    let new_root = short_hash(&format!("{:#}", sync_state_after.current_gsr));

    let mut inner = lock_runtime(&runtime);
    let apply_result = (|| {
        ensure_runtime_loaded(&mut inner, &objects_dir)?;

        let mut nullified_files = Vec::new();
        for object_id in &input.input_object_ids {
            let record = inner
                .objects
                .iter_mut()
                .find(|record| &record.id == object_id)
                .ok_or_else(|| format!("input object not found while finalizing: {object_id}"))?;
            if record.validity != RuntimeValidity::Live {
                return Err(format!("input object already nullified: {object_id}"));
            }
            record.validity = RuntimeValidity::Nullified;
            record.nullifier = Some(short_hash(&format!("{}:{}:null", object_id, old_root)));
            nullified_files.push(record.file_name.clone());
        }

        if spendable_outputs.objs.len() != descriptor.output_classes.len() {
            return Err(format!(
                "action {} output mismatch: descriptor expects {}, engine returned {}",
                input.action_id,
                descriptor.output_classes.len(),
                spendable_outputs.objs.len()
            ));
        }

        let mut output_files = Vec::new();
        for (index, class_name) in descriptor.output_classes.iter().enumerate() {
            let spendable = spendable_outputs.obj(index);
            let object_index = inner.next_object_index;
            let file_name = format_output_file_name(class_name, object_index);
            inner.next_object_index += 1;

            output_files.push(file_name.clone());
            inner.objects.push(RuntimeObjectRecord {
                id: format!("obj-{object_index}"),
                file_name,
                class_name: class_name.clone(),
                source_action: Some(input.action_id.clone()),
                validity: RuntimeValidity::Live,
                state_hash: short_hash(&format!("{:#}", spendable.obj.commitment())),
                nullifier: None,
                stats: stats_from_object(&spendable),
                spendable: Some(spendable),
            });
        }

        inner.state_root = sync_state_after.state_root;
        sync_object_files(&inner, &objects_dir)?;

        Ok(RunSdkActionResult {
            ok: true,
            old_root: old_root.clone(),
            new_root: new_root.clone(),
            output_files,
            nullified_files,
            objects: inner.objects.iter().map(to_inventory_item).collect(),
        })
    })();
    inner.run_in_progress = false;
    let result = apply_result?;

    emit_progress(
        &app,
        &RunSdkActionProgress {
            run_id: run_id,
            phase: "commit".to_string(),
            status: "done".to_string(),
            message: format!("Commit complete ({da_receipt})"),
            verify_index: None,
            detail: Some(result.new_root.clone()),
            old_root: Some(result.old_root.clone()),
            new_root: Some(result.new_root.clone()),
            output_file: result.output_files.first().cloned(),
        },
    )?;

    Ok(result)
}
