use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use base64::{engine::general_purpose::STANDARD, Engine};
use common::{
    blob::MAX_SIMPLE_BLOB_PAYLOAD_BYTES,
    payload::{Payload, PayloadProof},
    shrink::{shrink_compress_pod, ShrunkMainPodSetup},
};
use craft_sdk::{Helper, SpendableObject, SpendableObjects};
use hex::{FromHex, ToHex};
use pod2::middleware::{hash_values, Hash, Key, Params, Value};
use serde::{Deserialize, Serialize};
use tauri::Emitter;
use txlib::StateRoot;

use super::settings::get_app_settings;
use crate::{
    action_spec,
    app_paths,
    state::{CraftRuntime, CraftRuntimeInner, RuntimeObjectRecord, RuntimeValidity},
    types::{
        ClassMetaDto, InventoryItemDto, LoadGuiBootstrapResult, MethodArgDto, ObjectDataEntryDto,
        ObjectMethodDto, RecipeDto, RunSdkActionInput, RunSdkActionProgress, RunSdkActionResult,
        SourceActionMetaDto,
    },
};

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedStateRoot {
    block_number: i64,
    transactions: serde_json::Value,
    nullifiers: serde_json::Value,
    gsrs: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedObjectRecord {
    id: String,
    class_name: String,
    source_action: Option<String>,
    validity: String,
    state_hash: String,
    nullifier: Option<String>,
    pod: Option<serde_json::Value>,
    obj: Option<serde_json::Value>,
    tx_live: Option<serde_json::Value>,
    tx_nullifiers: Option<serde_json::Value>,
    tx_state_root: Option<PersistedStateRoot>,
}

#[derive(Debug, Deserialize)]
struct SynchronizerStateFullResponse {
    block_number: i64,
    transactions: Vec<String>,
    nullifiers: Vec<String>,
    gsrs: Vec<String>,
    current_gsr: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SynchronizerStateHeadResponse {
    current_gsr: Option<String>,
}

#[derive(Debug, Serialize)]
struct SynchronizerTxContainsRequest {
    tx_hashes: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct SynchronizerTxContainsResponse {
    results: Vec<SynchronizerTxContainsEntry>,
}

#[derive(Debug, Deserialize)]
struct SynchronizerTxContainsEntry {
    tx_hash: String,
    present: bool,
}

#[derive(Debug, Deserialize)]
struct SynchronizerTxStatusResponse {
    present: bool,
}

struct SynchronizerState {
    state_root: StateRoot,
    current_gsr: Hash,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum RelayerJobStatus {
    Queued,
    Sending,
    Submitted,
    Confirmed,
    Failed,
}

impl RelayerJobStatus {
    fn as_str(self) -> &'static str {
        match self {
            RelayerJobStatus::Queued => "queued",
            RelayerJobStatus::Sending => "sending",
            RelayerJobStatus::Submitted => "submitted",
            RelayerJobStatus::Confirmed => "confirmed",
            RelayerJobStatus::Failed => "failed",
        }
    }
}

#[derive(Debug, Serialize)]
struct RelayerSubmitRequest {
    payload_base64: String,
    client_ref: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RelayerSubmitResponse {
    job_id: String,
    status: RelayerJobStatus,
}

#[derive(Debug, Deserialize)]
struct RelayerJobStatusResponse {
    job_id: String,
    status: RelayerJobStatus,
    tx_hash: Option<String>,
    last_error: Option<String>,
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

fn relayer_poll_timeout_secs() -> u64 {
    std::env::var("RELAYER_POLL_TIMEOUT_SECS")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(180)
}

fn relayer_poll_interval_millis() -> u64 {
    std::env::var("RELAYER_POLL_INTERVAL_MS")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .filter(|value| *value >= 250)
        .unwrap_or(1500)
}

fn synchronizer_poll_timeout_secs() -> u64 {
    std::env::var("SYNCHRONIZER_POLL_TIMEOUT_SECS")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(120)
}

fn synchronizer_poll_interval_millis() -> u64 {
    std::env::var("SYNCHRONIZER_POLL_INTERVAL_MS")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .filter(|value| *value >= 250)
        .unwrap_or(1200)
}

fn ensure_non_empty_url(name: &str, value: String) -> Result<String, String> {
    let trimmed = value.trim().to_string();
    if trimmed.is_empty() {
        return Err(format!("{name} is empty"));
    }
    Ok(trimmed)
}

fn relayer_proofs_endpoint(relayer_api_url: &str) -> String {
    format!("{}/api/v1/proofs", relayer_api_url.trim_end_matches('/'))
}

fn relayer_proof_status_endpoint(relayer_api_url: &str, job_id: &str) -> String {
    format!(
        "{}/api/v1/proofs/{job_id}",
        relayer_api_url.trim_end_matches('/')
    )
}

fn submit_proof_to_relayer(
    relayer_api_url: &str,
    payload_bytes: &[u8],
    client_ref: Option<String>,
) -> Result<RelayerSubmitResponse, String> {
    if payload_bytes.len() > MAX_SIMPLE_BLOB_PAYLOAD_BYTES {
        return Err(format!(
            "payload exceeds single-blob limit: {} > {}",
            payload_bytes.len(),
            MAX_SIMPLE_BLOB_PAYLOAD_BYTES
        ));
    }

    let endpoint = relayer_proofs_endpoint(relayer_api_url);
    let request = RelayerSubmitRequest {
        payload_base64: STANDARD.encode(payload_bytes),
        client_ref,
    };

    let client = reqwest::blocking::Client::new();
    let response = client
        .post(&endpoint)
        .json(&request)
        .send()
        .map_err(|err| format!("failed to submit proof to relayer at {endpoint}: {err}"))?;

    let status = response.status();
    let body = response.text().unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "relayer submit failed with {} {}: {}",
            status.as_u16(),
            status,
            body
        ));
    }

    serde_json::from_str::<RelayerSubmitResponse>(&body)
        .map_err(|err| format!("failed to decode relayer submit response: {err}; body={body}"))
}

fn fetch_relayer_job_status(
    relayer_api_url: &str,
    job_id: &str,
) -> Result<RelayerJobStatusResponse, String> {
    let endpoint = relayer_proof_status_endpoint(relayer_api_url, job_id);
    let client = reqwest::blocking::Client::new();
    let response = client
        .get(&endpoint)
        .send()
        .map_err(|err| format!("failed to query relayer job at {endpoint}: {err}"))?;

    let status = response.status();
    let body = response.text().unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "relayer status failed with {} {}: {}",
            status.as_u16(),
            status,
            body
        ));
    }

    serde_json::from_str::<RelayerJobStatusResponse>(&body)
        .map_err(|err| format!("failed to decode relayer status response: {err}; body={body}"))
}

fn wait_for_relayer_confirmation(
    relayer_api_url: &str,
    job_id: &str,
    timeout_secs: u64,
    poll_interval_ms: u64,
) -> Result<RelayerJobStatusResponse, String> {
    let timeout = Duration::from_secs(timeout_secs);
    let poll_interval = Duration::from_millis(poll_interval_ms);
    let start = Instant::now();

    loop {
        let status = fetch_relayer_job_status(relayer_api_url, job_id)?;
        match status.status {
            RelayerJobStatus::Confirmed => return Ok(status),
            RelayerJobStatus::Failed => {
                return Err(format!(
                    "relayer job {} failed: {}",
                    status.job_id,
                    status
                        .last_error
                        .clone()
                        .unwrap_or_else(|| "unknown error".to_string())
                ));
            }
            RelayerJobStatus::Queued | RelayerJobStatus::Sending | RelayerJobStatus::Submitted => {}
        }

        if start.elapsed() >= timeout {
            return Err(format!(
                "timed out waiting for relayer job {} after {}s",
                job_id, timeout_secs
            ));
        }
        std::thread::sleep(poll_interval);
    }
}

fn parse_hash_hex(value: &str) -> Result<Hash, String> {
    let trimmed = value.trim().strip_prefix("0x").unwrap_or(value.trim());
    Hash::from_hex(trimmed).map_err(|err| format!("invalid hash {value}: {err}"))
}

fn encode_hash_hex(hash: &Hash) -> String {
    format!("0x{}", hash.encode_hex::<String>())
}

fn synchronizer_state_head_endpoint(sync_api_url: &str) -> String {
    format!("{}/v1/state/head", sync_api_url.trim_end_matches('/'))
}

fn synchronizer_state_full_endpoint(sync_api_url: &str) -> String {
    format!("{}/v1/state/full", sync_api_url.trim_end_matches('/'))
}

fn synchronizer_state_tx_contains_endpoint(sync_api_url: &str) -> String {
    format!(
        "{}/v1/state/tx/contains",
        sync_api_url.trim_end_matches('/')
    )
}

fn synchronizer_state_tx_endpoint(sync_api_url: &str, tx_hash: &Hash) -> String {
    format!(
        "{}/v1/state/tx/{}",
        sync_api_url.trim_end_matches('/'),
        encode_hash_hex(tx_hash)
    )
}

fn fetch_synchronizer_head(sync_api_url: &str) -> Result<Option<Hash>, String> {
    let endpoint = synchronizer_state_head_endpoint(sync_api_url);
    let response = reqwest::blocking::get(&endpoint)
        .map_err(|err| format!("failed to query synchronizer at {endpoint}: {err}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "synchronizer request failed: {} {}",
            response.status().as_u16(),
            response.status()
        ));
    }

    let payload: SynchronizerStateHeadResponse = response
        .json()
        .map_err(|err| format!("failed to decode synchronizer head response: {err}"))?;
    payload
        .current_gsr
        .as_deref()
        .map(parse_hash_hex)
        .transpose()
}

fn fetch_synchronizer_state(sync_api_url: &str) -> Result<SynchronizerState, String> {
    let endpoint = synchronizer_state_full_endpoint(sync_api_url);
    let response = reqwest::blocking::get(&endpoint)
        .map_err(|err| format!("failed to query synchronizer at {endpoint}: {err}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "synchronizer request failed: {} {}",
            response.status().as_u16(),
            response.status()
        ));
    }
    let payload: SynchronizerStateFullResponse = response
        .json()
        .map_err(|err| format!("failed to decode synchronizer full state response: {err}"))?;

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
    let gsrs = payload
        .gsrs
        .iter()
        .map(|entry| parse_hash_hex(entry))
        .collect::<Result<Vec<_>, String>>()?;

    let state_root = StateRoot::new(payload.block_number, &transactions, &nullifiers, &gsrs);
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

fn fetch_synchronizer_tx_contains(
    sync_api_url: &str,
    tx_hashes: &[Hash],
) -> Result<HashSet<Hash>, String> {
    if tx_hashes.is_empty() {
        return Ok(HashSet::new());
    }

    let endpoint = synchronizer_state_tx_contains_endpoint(sync_api_url);
    let request = SynchronizerTxContainsRequest {
        tx_hashes: tx_hashes.iter().map(encode_hash_hex).collect(),
    };
    let client = reqwest::blocking::Client::new();
    let response = client
        .post(&endpoint)
        .json(&request)
        .send()
        .map_err(|err| format!("failed to query synchronizer at {endpoint}: {err}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "synchronizer request failed: {} {}",
            response.status().as_u16(),
            response.status()
        ));
    }

    let payload: SynchronizerTxContainsResponse = response
        .json()
        .map_err(|err| format!("failed to decode synchronizer tx/contains response: {err}"))?;
    let mut present = HashSet::new();
    for entry in payload.results {
        if entry.present {
            present.insert(parse_hash_hex(&entry.tx_hash)?);
        }
    }
    Ok(present)
}

fn fetch_synchronizer_tx_status(
    sync_api_url: &str,
    tx_hash: &Hash,
) -> Result<SynchronizerTxStatusResponse, String> {
    let endpoint = synchronizer_state_tx_endpoint(sync_api_url, tx_hash);
    let response = reqwest::blocking::get(&endpoint)
        .map_err(|err| format!("failed to query synchronizer at {endpoint}: {err}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "synchronizer request failed: {} {}",
            response.status().as_u16(),
            response.status()
        ));
    }

    response
        .json::<SynchronizerTxStatusResponse>()
        .map_err(|err| format!("failed to decode synchronizer tx status response: {err}"))
}

fn wait_for_synchronizer_tx(
    sync_api_url: &str,
    tx_final: Hash,
    timeout_secs: u64,
    poll_interval_ms: u64,
) -> Result<SynchronizerState, String> {
    let timeout = Duration::from_secs(timeout_secs);
    let poll_interval = Duration::from_millis(poll_interval_ms);
    let start = Instant::now();
    loop {
        let status = fetch_synchronizer_tx_status(sync_api_url, &tx_final)?;
        if status.present {
            return fetch_synchronizer_state(sync_api_url);
        }
        if start.elapsed() >= timeout {
            return Err(format!(
                "synchronizer did not index relayed tx {} within {}s",
                encode_hash_hex(&tx_final),
                timeout_secs
            ));
        }
        std::thread::sleep(poll_interval);
    }
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

fn object_id_from_spendable(spendable: &SpendableObject) -> String {
    format!("{:#}", spendable.obj.commitment())
}

fn object_state_hash_from_spendable(spendable: &SpendableObject) -> String {
    format!("{:#}", spendable.obj.commitment())
}

fn object_nullifier_from_spendable(spendable: &SpendableObject) -> Result<String, String> {
    let object_key = spendable
        .obj
        .get(&Key::from("key"))
        .cloned()
        .map_err(|err| {
            format!(
                "input object missing required key field for {}: {err}",
                object_id_from_spendable(spendable),
            )
        })?;
    let object_key_hash = hash_values(&[Value::from(spendable.obj.commitment()), object_key]);
    let object_nullifier = hash_values(&[
        Value::from(object_key_hash),
        Value::from("txlib-nullifier-v1"),
    ]);
    Ok(encode_hash_hex(&object_nullifier))
}

const NULLIFIED_DIR_NAME: &str = ".nullified";

fn nullified_objects_dir(objects_dir: &Path) -> PathBuf {
    objects_dir.join(NULLIFIED_DIR_NAME)
}

fn value_string(raw: String) -> String {
    let trimmed = raw.trim();
    let unquoted = if trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2 {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    };

    if let Some(raw_inner) = unquoted
        .strip_prefix("Raw(")
        .and_then(|value| value.strip_suffix(')'))
    {
        raw_inner.trim().to_string()
    } else {
        unquoted.to_string()
    }
}

fn object_data_from_object(spendable: &SpendableObject) -> Vec<(String, String)> {
    let mut data = Vec::new();
    for (key, value) in spendable.obj.kvs() {
        data.push((key.name().to_string(), value_string(format!("{value}"))));
    }
    data.sort_by(|a, b| a.0.cmp(&b.0));
    data
}

fn action_descriptors_by_name() -> HashMap<String, action_spec::ActionDescriptor> {
    action_spec::action_descriptors_by_name()
}

fn build_action_catalog() -> Vec<RecipeDto> {
    action_spec::visible_action_descriptors()
        .into_iter()
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
    let class_ui = action_spec::class_ui_meta(&record.class_name);
    let obj_data = record
        .spendable
        .as_ref()
        .map(object_data_from_object)
        .unwrap_or_default();
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
        source_action: record
            .source_action
            .as_ref()
            .map(|name| SourceActionMetaDto {
                name: name.clone(),
                hash: short_hash(name),
            }),
        description: Some(class_ui.description.to_string()),
        methods: Vec::<ObjectMethodDto>::new(),
        obj: obj_data
            .iter()
            .map(|(key, value)| ObjectDataEntryDto {
                key: key.clone(),
                value: value.clone(),
            })
            .collect(),
    }
}

fn persist_state_root(state_root: &StateRoot) -> Result<PersistedStateRoot, String> {
    Ok(PersistedStateRoot {
        block_number: state_root.block_number,
        transactions: serde_json::to_value(&state_root.transactions)
            .map_err(|err| format!("failed to serialize state_root.transactions: {err}"))?,
        nullifiers: serde_json::to_value(&state_root.nullifiers)
            .map_err(|err| format!("failed to serialize state_root.nullifiers: {err}"))?,
        gsrs: serde_json::to_value(&state_root.gsrs)
            .map_err(|err| format!("failed to serialize state_root.gsrs: {err}"))?,
    })
}

fn restore_state_root(data: PersistedStateRoot) -> Result<StateRoot, String> {
    Ok(StateRoot {
        block_number: data.block_number,
        transactions: serde_json::from_value(data.transactions)
            .map_err(|err| format!("failed to deserialize state_root.transactions: {err}"))?,
        nullifiers: serde_json::from_value(data.nullifiers)
            .map_err(|err| format!("failed to deserialize state_root.nullifiers: {err}"))?,
        gsrs: serde_json::from_value(data.gsrs)
            .map_err(|err| format!("failed to deserialize state_root.gsrs: {err}"))?,
    })
}

fn persist_spendable(
    spendable: &SpendableObject,
) -> Result<
    (
        serde_json::Value,
        serde_json::Value,
        serde_json::Value,
        serde_json::Value,
        PersistedStateRoot,
    ),
    String,
> {
    Ok((
        serde_json::to_value(&spendable.pod)
            .map_err(|err| format!("failed to serialize spendable.pod: {err}"))?,
        serde_json::to_value(&spendable.obj)
            .map_err(|err| format!("failed to serialize spendable.obj: {err}"))?,
        serde_json::to_value(&spendable.tx.live)
            .map_err(|err| format!("failed to serialize spendable.tx.live: {err}"))?,
        serde_json::to_value(&spendable.tx.nullifiers)
            .map_err(|err| format!("failed to serialize spendable.tx.nullifiers: {err}"))?,
        persist_state_root(spendable.tx.state_root.as_ref())?,
    ))
}

fn restore_spendable(
    pod: Option<serde_json::Value>,
    obj: Option<serde_json::Value>,
    tx_live: Option<serde_json::Value>,
    tx_nullifiers: Option<serde_json::Value>,
    tx_state_root: Option<PersistedStateRoot>,
) -> Result<Option<SpendableObject>, String> {
    match (pod, obj, tx_live, tx_nullifiers, tx_state_root) {
        (None, None, None, None, None) => Ok(None),
        (Some(pod), Some(obj), Some(tx_live), Some(tx_nullifiers), Some(tx_state_root)) => {
            let state_root = restore_state_root(tx_state_root)?;
            let tx = txlib::Tx {
                live: serde_json::from_value(tx_live)
                    .map_err(|err| format!("failed to deserialize spendable.tx.live: {err}"))?,
                nullifiers: serde_json::from_value(tx_nullifiers).map_err(|err| {
                    format!("failed to deserialize spendable.tx.nullifiers: {err}")
                })?,
                state_root: Arc::new(state_root),
            };
            Ok(Some(SpendableObject {
                pod: serde_json::from_value(pod)
                    .map_err(|err| format!("failed to deserialize spendable.pod: {err}"))?,
                obj: serde_json::from_value(obj)
                    .map_err(|err| format!("failed to deserialize spendable.obj: {err}"))?,
                tx,
            }))
        }
        _ => Err(
            "invalid object file: spendable fields must all be present or all absent".to_string(),
        ),
    }
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
    file_name: &str,
) -> Result<RuntimeObjectRecord, String> {
    let spendable = restore_spendable(
        record.pod,
        record.obj,
        record.tx_live,
        record.tx_nullifiers,
        record.tx_state_root,
    )?;
    let state_hash = spendable
        .as_ref()
        .map(object_state_hash_from_spendable)
        .unwrap_or(record.state_hash);
    Ok(RuntimeObjectRecord {
        id: record.id,
        file_name: file_name.to_string(),
        class_name: record.class_name,
        source_action: record.source_action,
        validity: validity_from_str(&record.validity, "object file")?,
        state_hash,
        nullifier: record.nullifier,
        spendable,
    })
}

fn parse_object_file(contents: &str, file_name: &str) -> Result<RuntimeObjectRecord, String> {
    let record = serde_json::from_str::<PersistedObjectRecord>(contents)
        .map_err(|err| format!("failed to parse {file_name} as object file: {err}"))?;
    restore_object_record(record, file_name)
}

fn persist_object_record(record: &RuntimeObjectRecord) -> Result<PersistedObjectRecord, String> {
    let (pod, obj, tx_live, tx_nullifiers, tx_state_root) =
        if let Some(spendable) = record.spendable.as_ref() {
            let (pod, obj, tx_live, tx_nullifiers, tx_state_root) = persist_spendable(spendable)?;
            (
                Some(pod),
                Some(obj),
                Some(tx_live),
                Some(tx_nullifiers),
                Some(tx_state_root),
            )
        } else {
            (None, None, None, None, None)
        };
    let state_hash = record
        .spendable
        .as_ref()
        .map(object_state_hash_from_spendable)
        .unwrap_or_else(|| record.state_hash.clone());
    Ok(PersistedObjectRecord {
        id: record.id.clone(),
        class_name: record.class_name.clone(),
        source_action: record.source_action.clone(),
        validity: match record.validity {
            RuntimeValidity::Live => "live".to_string(),
            RuntimeValidity::Nullified => "nullified".to_string(),
        },
        state_hash,
        nullifier: record.nullifier.clone(),
        pod,
        obj,
        tx_live,
        tx_nullifiers,
        tx_state_root,
    })
}

fn sync_object_files(inner: &CraftRuntimeInner, objects_dir: &Path) -> Result<(), String> {
    fs::create_dir_all(objects_dir)
        .map_err(|err| format!("failed to create objects directory: {err}"))?;
    let nullified_dir = nullified_objects_dir(objects_dir);
    fs::create_dir_all(&nullified_dir)
        .map_err(|err| format!("failed to create nullified directory: {err}"))?;

    for record in &inner.objects {
        let persisted = persist_object_record(record)?;
        let serialized = serde_json::to_string_pretty(&persisted).map_err(|err| {
            format!(
                "failed to serialize object file {}: {err}",
                record.file_name
            )
        })?;
        let target_path = match record.validity {
            RuntimeValidity::Live => objects_dir.join(&record.file_name),
            RuntimeValidity::Nullified => nullified_dir.join(&record.file_name),
        };
        fs::write(&target_path, serialized)
            .map_err(|err| format!("failed to write object file {}: {err}", record.file_name))?;

        let stale_path = match record.validity {
            RuntimeValidity::Live => nullified_dir.join(&record.file_name),
            RuntimeValidity::Nullified => objects_dir.join(&record.file_name),
        };
        if stale_path != target_path {
            match fs::remove_file(&stale_path) {
                Ok(_) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => {
                    eprintln!(
                        "zk-craft: failed to remove stale object file {}: {err}",
                        stale_path.display()
                    );
                }
            }
        }
    }

    Ok(())
}

fn load_object_files_from_dir(
    objects: &mut HashMap<String, (RuntimeObjectRecord, u8)>,
    source_dir: &Path,
    in_nullified_dir: bool,
) -> Result<(), String> {
    if !source_dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(source_dir)
        .map_err(|err| format!("failed to read objects directory: {err}"))?
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
            Ok(record) => {
                let score = if (in_nullified_dir
                    && matches!(record.validity, RuntimeValidity::Nullified))
                    || (!in_nullified_dir && matches!(record.validity, RuntimeValidity::Live))
                {
                    2
                } else {
                    1
                };
                let replace = objects
                    .get(&record.file_name)
                    .map(|(_, existing_score)| score > *existing_score)
                    .unwrap_or(true);
                if replace {
                    objects.insert(record.file_name.clone(), (record, score));
                }
            }
            Err(err) => eprintln!("zk-craft: failed to parse {file_name}, skipping: {err}"),
        }
    }

    Ok(())
}

fn load_object_files(objects_dir: &Path) -> Result<Vec<RuntimeObjectRecord>, String> {
    let mut records_by_file = HashMap::<String, (RuntimeObjectRecord, u8)>::new();
    load_object_files_from_dir(&mut records_by_file, objects_dir, false)?;
    load_object_files_from_dir(
        &mut records_by_file,
        &nullified_objects_dir(objects_dir),
        true,
    )?;

    let mut objects = records_by_file
        .into_values()
        .map(|(record, _)| record)
        .collect::<Vec<_>>();
    objects.sort_by(|a, b| a.file_name.cmp(&b.file_name));
    Ok(objects)
}

fn next_object_index_from_records(objects: &[RuntimeObjectRecord]) -> u64 {
    let max_index = objects
        .iter()
        .filter_map(|record| {
            let without_ext = record.file_name.strip_suffix(".dobj")?;
            let (_prefix, suffix) = without_ext.rsplit_once('_')?;
            suffix.parse::<u64>().ok()
        })
        .max()
        .unwrap_or(0);
    max_index + 1
}

fn refresh_runtime_objects(
    inner: &mut CraftRuntimeInner,
    objects_dir: &Path,
) -> Result<(), String> {
    inner.objects = load_object_files(objects_dir)?;
    inner.next_object_index = next_object_index_from_records(&inner.objects);
    Ok(())
}

fn ensure_runtime_loaded(inner: &mut CraftRuntimeInner, objects_dir: &Path) -> Result<(), String> {
    if inner.loaded {
        return Ok(());
    }
    fs::create_dir_all(objects_dir)
        .map_err(|err| format!("failed to create objects directory: {err}"))?;
    fs::create_dir_all(nullified_objects_dir(objects_dir))
        .map_err(|err| format!("failed to create nullified directory: {err}"))?;
    refresh_runtime_objects(inner, objects_dir)?;
    inner.state_root = empty_state_root();
    inner.loaded = true;
    Ok(())
}

fn emit_progress(app: &tauri::AppHandle, payload: &RunSdkActionProgress) -> Result<(), String> {
    app.emit("run-sdk-action-progress", payload)
        .map_err(|err| format!("failed to emit run progress: {err}"))
}

fn parse_object_file_from_path(path: &Path) -> Result<RuntimeObjectRecord, String> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("invalid input path (missing file name): {}", path.display()))?;
    let contents = fs::read_to_string(path)
        .map_err(|err| format!("failed to read input file {}: {err}", path.display()))?;
    parse_object_file(&contents, file_name)
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
    let objects_dir = app_paths::objects_dir(&app)?;
    let actions = build_action_catalog();
    let effective_urls = get_app_settings(app.clone())?;
    let sync_head = fetch_synchronizer_head(&effective_urls.synchronizer_api_url);
    let mut inner = lock_runtime(&runtime);
    if let Err(err) = ensure_runtime_loaded(&mut inner, &objects_dir) {
        eprintln!("zk-craft: bootstrap runtime failed, resetting state: {err}");
        inner.next_object_index = 1;
        inner.state_root = empty_state_root();
        inner.objects.clear();
        inner.loaded = true;
        let _ = sync_object_files(&inner, &objects_dir);
    }
    if !inner.run_in_progress {
        if let Err(err) = refresh_runtime_objects(&mut inner, &objects_dir) {
            eprintln!("zk-craft: failed to refresh objects from disk: {err}");
        }
    }
    if let Err(err) = sync_head {
        eprintln!("zk-craft: synchronizer unavailable during bootstrap: {err}");
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
    let helper = Helper::new(action_spec::dependencies(), action_spec::actions());
    // Relayed payloads are recursively verified/compressed, which is incompatible with MockMainPod.
    let builder = helper.builder(false, Arc::new(state_root));
    Ok(builder.action(&action_id, inputs))
}

fn build_relayer_payload(
    old_state_root_hash: &Hash,
    action_output: &SpendableObjects,
) -> Result<Vec<u8>, String> {
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
        nullifiers,
    };

    let payload_bytes = payload.to_bytes();
    Ok(payload_bytes)
}

#[tauri::command]
pub async fn run_sdk_action(
    app: tauri::AppHandle,
    runtime: tauri::State<'_, CraftRuntime>,
    input: RunSdkActionInput,
) -> Result<RunSdkActionResult, String> {
    let objects_dir = app_paths::objects_dir(&app)?;
    let descriptors = action_descriptors_by_name();
    let descriptor = descriptors
        .get(&input.action_id)
        .ok_or_else(|| format!("unknown action: {}", input.action_id))?;
    if descriptor.hidden {
        return Err(format!(
            "action is internal and cannot be run directly: {}",
            input.action_id
        ));
    }

    if input.inputs.len() != descriptor.input_classes.len() {
        return Err(format!(
            "{} expects {} inputs, got {}",
            input.action_id,
            descriptor.input_classes.len(),
            input.inputs.len()
        ));
    }

    let mut seen_paths = HashSet::new();
    for arg in &input.inputs {
        let object_path = arg.object_path.trim();
        if object_path.is_empty() {
            return Err("each input must include objectPath".to_string());
        }
        if !seen_paths.insert(object_path.to_string()) {
            return Err(format!(
                "duplicate input object path is not allowed: {object_path}"
            ));
        }
    }

    let effective_urls = get_app_settings(app.clone())?;
    let sync_api_url =
        ensure_non_empty_url("SYNCHRONIZER_API_URL", effective_urls.synchronizer_api_url)?;
    let sync_state = fetch_synchronizer_state(&sync_api_url)?;
    let state_root_for_run = sync_state.state_root.clone();
    let old_root_hash = sync_state.current_gsr;
    let old_root = short_hash(&format!("{:#}", old_root_hash));
    let relayer_url = ensure_non_empty_url("RELAYER_API_URL", effective_urls.relayer_api_url)?;
    let relayer_timeout_secs = relayer_poll_timeout_secs();
    let relayer_poll_interval_ms = relayer_poll_interval_millis();
    let sync_wait_timeout_secs = synchronizer_poll_timeout_secs();
    let sync_wait_interval_ms = synchronizer_poll_interval_millis();

    struct ResolvedRunInput {
        id: String,
        file_name: String,
        class_name: String,
        source_action: Option<String>,
        state_hash: String,
        nullifier: String,
    }

    let (input_spendables, verify_targets, resolved_inputs, source_tx_hashes);
    {
        let mut inner = lock_runtime(&runtime);
        ensure_runtime_loaded(&mut inner, &objects_dir)?;

        if inner.run_in_progress {
            return Err("another action run is already in progress".to_string());
        }
        refresh_runtime_objects(&mut inner, &objects_dir)?;
        inner.state_root = state_root_for_run.clone();

        let mut collected_spendables = Vec::new();
        let mut collected_targets = Vec::new();
        let mut collected_resolved = Vec::new();

        for (slot, arg) in input.inputs.iter().enumerate() {
            let expected_class = descriptor.input_classes[slot].as_str();
            let object_path = arg.object_path.trim();
            if object_path.is_empty() {
                return Err(format!("missing objectPath for input slot {}", slot + 1));
            }

            let path_ref = Path::new(object_path);
            let record = parse_object_file_from_path(path_ref)?;
            let target_label = arg
                .label
                .clone()
                .unwrap_or_else(|| record.file_name.clone());

            if record.validity != RuntimeValidity::Live {
                return Err(format!("input object is not live: {}", record.id));
            }
            if record.class_name != expected_class {
                return Err(format!(
                    "input class mismatch for {}: expected {}, got {}",
                    record.id, expected_class, record.class_name
                ));
            }

            let spendable = record
                .spendable
                .as_ref()
                .ok_or_else(|| format!("input object missing spendable witness: {}", record.id))?;
            let input_nullifier = object_nullifier_from_spendable(spendable)?;
            collected_spendables.push(clone_spendable(spendable));
            collected_targets.push(target_label);
            collected_resolved.push(ResolvedRunInput {
                id: record.id.clone(),
                file_name: record.file_name.clone(),
                class_name: record.class_name.clone(),
                source_action: record.source_action.clone(),
                state_hash: record.state_hash.clone(),
                nullifier: input_nullifier,
            });
        }

        let collected_source_tx_hashes = collected_spendables
            .iter()
            .map(|spendable| spendable.tx.dict().commitment())
            .collect::<Vec<_>>();

        input_spendables = collected_spendables;
        verify_targets = collected_targets;
        resolved_inputs = collected_resolved;
        source_tx_hashes = collected_source_tx_hashes;
    }

    let grounded_txs = fetch_synchronizer_tx_contains(&sync_api_url, &source_tx_hashes)?;
    let mut missing_grounding = Vec::new();
    for (index, tx_hash) in source_tx_hashes.iter().enumerate() {
        if !grounded_txs.contains(tx_hash) {
            let label = verify_targets
                .get(index)
                .cloned()
                .unwrap_or_else(|| format!("input-{index}"));
            missing_grounding.push(format!("{} -> {}", label, encode_hash_hex(tx_hash)));
        }
    }
    if !missing_grounding.is_empty() {
        return Err(format!(
            "inputs not yet synchronized; wait and retry: {}",
            missing_grounding.join(", ")
        ));
    }

    {
        let mut inner = lock_runtime(&runtime);
        if inner.run_in_progress {
            return Err("another action run is already in progress".to_string());
        }
        inner.run_in_progress = true;
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

    let payload_bytes = match build_relayer_payload(&old_root_hash, &spendable_outputs) {
        Ok(payload) => payload,
        Err(err) => {
            clear_run_in_progress(&runtime);
            return Err(err);
        }
    };
    let expected_tx_final = spendable_outputs.tx.dict().commitment();

    emit_progress(
        &app,
        &RunSdkActionProgress {
            run_id: run_id.clone(),
            phase: "commit".to_string(),
            status: "running".to_string(),
            message: "Submitting proof to relayer".to_string(),
            verify_index: None,
            detail: Some("submit".to_string()),
            old_root: Some(old_root.clone()),
            new_root: None,
            output_file: None,
        },
    )?;

    let relayer_url_for_submit = relayer_url.clone();
    let action_ref = input.action_id.clone();
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
        Ok(Err(err)) => {
            clear_run_in_progress(&runtime);
            return Err(err);
        }
        Err(err) => {
            clear_run_in_progress(&runtime);
            return Err(format!("failed while submitting proof to relayer: {err}"));
        }
    };
    let submitted_job_id = submit_response.job_id.clone();
    let submitted_status = submit_response.status;
    if submitted_status == RelayerJobStatus::Failed {
        clear_run_in_progress(&runtime);
        return Err(format!(
            "relayer rejected job {} immediately",
            submitted_job_id
        ));
    }

    emit_progress(
        &app,
        &RunSdkActionProgress {
            run_id: run_id.clone(),
            phase: "commit".to_string(),
            status: "running".to_string(),
            message: format!("Waiting for relayer job {submitted_job_id}"),
            verify_index: None,
            detail: Some(format!("status: {}", submitted_status.as_str())),
            old_root: Some(old_root.clone()),
            new_root: None,
            output_file: None,
        },
    )?;

    let relayer_url_for_wait = relayer_url.clone();
    let job_id_for_wait = submitted_job_id.clone();
    let wait_job = tauri::async_runtime::spawn_blocking(move || {
        wait_for_relayer_confirmation(
            &relayer_url_for_wait,
            &job_id_for_wait,
            relayer_timeout_secs,
            relayer_poll_interval_ms,
        )
    })
    .await;
    let relay_status = match wait_job {
        Ok(Ok(status)) => status,
        Ok(Err(err)) => {
            clear_run_in_progress(&runtime);
            return Err(err);
        }
        Err(err) => {
            clear_run_in_progress(&runtime);
            return Err(format!("failed while polling relayer job status: {err}"));
        }
    };
    let da_receipt = relay_status
        .tx_hash
        .clone()
        .unwrap_or_else(|| format!("job {}", relay_status.job_id));

    let sync_state_after = match wait_for_synchronizer_tx(
        &sync_api_url,
        expected_tx_final,
        sync_wait_timeout_secs,
        sync_wait_interval_ms,
    ) {
        Ok(state) => state,
        Err(err) => {
            clear_run_in_progress(&runtime);
            return Err(format!(
                "failed to observe relayed tx in synchronizer after relay confirmation: {err}"
            ));
        }
    };
    let new_root = short_hash(&format!("{:#}", sync_state_after.current_gsr));

    let mut inner = lock_runtime(&runtime);
    let apply_result =
        (|| {
            ensure_runtime_loaded(&mut inner, &objects_dir)?;

            let mut nullified_files = Vec::new();
            for resolved in &resolved_inputs {
                if let Some(record) = inner.objects.iter_mut().find(|record| {
                    record.id == resolved.id && record.file_name == resolved.file_name
                }) {
                    if record.validity != RuntimeValidity::Live {
                        return Err(format!("input object already nullified: {}", resolved.id));
                    }
                    record.validity = RuntimeValidity::Nullified;
                    record.nullifier = Some(resolved.nullifier.clone());
                    nullified_files.push(record.file_name.clone());
                } else {
                    inner.objects.push(RuntimeObjectRecord {
                        id: resolved.id.clone(),
                        file_name: resolved.file_name.clone(),
                        class_name: resolved.class_name.clone(),
                        source_action: resolved.source_action.clone(),
                        validity: RuntimeValidity::Nullified,
                        state_hash: resolved.state_hash.clone(),
                        nullifier: Some(resolved.nullifier.clone()),
                        spendable: None,
                    });
                    nullified_files.push(resolved.file_name.clone());
                }
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
                let object_id = object_id_from_spendable(&spendable);

                output_files.push(file_name.clone());
                inner.objects.push(RuntimeObjectRecord {
                    id: object_id,
                    file_name,
                    class_name: class_name.clone(),
                    source_action: Some(input.action_id.clone()),
                    validity: RuntimeValidity::Live,
                    state_hash: object_state_hash_from_spendable(&spendable),
                    nullifier: None,
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
            run_id,
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
