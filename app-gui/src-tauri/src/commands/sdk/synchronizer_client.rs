use std::{
    collections::HashSet,
    time::{Duration, Instant},
};

use hex::{FromHex, ToHex};
use pod2::middleware::Hash;
use serde::{Deserialize, Serialize};
use txlib::StateRoot;

pub(super) const SYNCHRONIZER_POLL_TIMEOUT_SECS: u64 = 120;
pub(super) const SYNCHRONIZER_POLL_INTERVAL_MS: u64 = 1200;

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

pub(super) struct SynchronizerState {
    pub(super) state_root: StateRoot,
    pub(super) current_gsr: Hash,
}

fn parse_hash_hex(value: &str) -> Result<Hash, String> {
    let trimmed = value.trim().strip_prefix("0x").unwrap_or(value.trim());
    Hash::from_hex(trimmed).map_err(|err| format!("invalid hash {value}: {err}"))
}

pub(super) fn encode_hash_hex(hash: &Hash) -> String {
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

pub(super) fn fetch_synchronizer_head(sync_api_url: &str) -> Result<Option<Hash>, String> {
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

pub(super) fn fetch_synchronizer_state(sync_api_url: &str) -> Result<SynchronizerState, String> {
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

pub(super) fn fetch_synchronizer_tx_contains(
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

pub(super) fn wait_for_synchronizer_tx(
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
