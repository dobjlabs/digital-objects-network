use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use anyhow::{anyhow, Result};
use hex::FromHex;
use pod2::middleware::Hash;
use synchronizer::api_types::{
    GroundingWitnessRequest, GroundingWitnessResponse, MembershipRequest, MembershipResponse,
    StateHeadResponse, TxStatusResponse,
};
use txlib::{GroundingWitness, StateRoot};

pub(crate) const SYNCHRONIZER_POLL_TIMEOUT_SECS: u64 = 120;
pub(crate) const SYNCHRONIZER_POLL_INTERVAL_MS: u64 = 1200;
pub(crate) use common::encode_hash_hex;

pub(crate) struct SynchronizerHead {
    pub(crate) current_gsr: Hash,
}

pub(crate) struct SynchronizerMembership {
    pub(crate) grounded_txs: HashSet<Hash>,
    pub(crate) on_chain_nullifiers: HashSet<Hash>,
}

fn parse_hash_hex(value: &str) -> Result<Hash> {
    let trimmed = value.trim().strip_prefix("0x").unwrap_or(value.trim());
    Hash::from_hex(trimmed).map_err(|err| anyhow!("invalid hash {value}: {err}"))
}

pub(crate) fn fetch_synchronizer_head(sync_api_url: &str) -> Result<SynchronizerHead> {
    let endpoint = format!("{}/v1/state/head", sync_api_url.trim_end_matches('/'));
    let response = reqwest::blocking::get(&endpoint)
        .map_err(|err| anyhow!("failed to query synchronizer at {endpoint}: {err}"))?;
    if !response.status().is_success() {
        return Err(anyhow!(
            "synchronizer request failed: {} {}",
            response.status().as_u16(),
            response.status()
        ));
    }
    let payload: StateHeadResponse = response
        .json()
        .map_err(|err| anyhow!("failed to decode synchronizer head response: {err}"))?;
    let current_gsr = payload
        .current_gsr
        .as_deref()
        .ok_or_else(|| anyhow!("synchronizer has no canonical grounded state yet"))
        .and_then(parse_hash_hex)?;
    Ok(SynchronizerHead { current_gsr })
}

pub(crate) fn fetch_grounding_witness(
    sync_api_url: &str,
    source_tx_hashes: &[Hash],
) -> Result<GroundingWitness> {
    let endpoint = format!(
        "{}/v1/txlib/grounding-witness",
        sync_api_url.trim_end_matches('/')
    );
    let request = GroundingWitnessRequest {
        source_tx_hashes: source_tx_hashes.iter().map(encode_hash_hex).collect(),
    };
    let client = reqwest::blocking::Client::new();
    let response = client
        .post(&endpoint)
        .json(&request)
        .send()
        .map_err(|err| anyhow!("failed to query synchronizer at {endpoint}: {err}"))?;
    if !response.status().is_success() {
        return Err(anyhow!(
            "synchronizer request failed: {} {}",
            response.status().as_u16(),
            response.status()
        ));
    }

    let payload: GroundingWitnessResponse = response.json().map_err(|err| {
        anyhow!("failed to decode synchronizer grounding witness response: {err}")
    })?;

    let state_root = StateRoot::new(
        payload.block_number,
        parse_hash_hex(&payload.transactions_root)?,
        parse_hash_hex(&payload.nullifiers_root)?,
        parse_hash_hex(&payload.gsrs_root)?,
    );
    let remote_state_root_hash = parse_hash_hex(&payload.state_root_hash)?;
    let derived_state_root_hash = state_root.hash();
    if remote_state_root_hash != derived_state_root_hash {
        return Err(anyhow!(
            "synchronizer grounding witness hash mismatch: remote={} derived={}",
            encode_hash_hex(&remote_state_root_hash),
            encode_hash_hex(&derived_state_root_hash)
        ));
    }

    let mut source_tx_proofs = HashMap::new();
    let mut missing = Vec::new();
    for entry in payload.source_tx_proofs {
        let tx_hash = parse_hash_hex(&entry.tx_hash)?;
        if entry.present {
            source_tx_proofs.insert(tx_hash, entry.proof);
        } else {
            missing.push(tx_hash);
        }
    }
    if !missing.is_empty() {
        let rendered = missing
            .iter()
            .map(encode_hash_hex)
            .collect::<Vec<_>>()
            .join(", ");
        return Err(anyhow!(
            "input not yet synchronized; wait and retry: {}",
            rendered
        ));
    }

    Ok(GroundingWitness::new(state_root, source_tx_proofs))
}

pub(crate) fn fetch_synchronizer_membership_with_nullifiers(
    sync_api_url: &str,
    tx_hashes: &[Hash],
    nullifiers: &[Hash],
) -> Result<SynchronizerMembership> {
    let endpoint = format!("{}/v1/state/membership", sync_api_url.trim_end_matches('/'));
    let request = MembershipRequest {
        tx_hashes: tx_hashes.iter().map(encode_hash_hex).collect(),
        nullifiers: nullifiers.iter().map(encode_hash_hex).collect(),
    };
    let client = reqwest::blocking::Client::new();
    let response = client
        .post(&endpoint)
        .json(&request)
        .send()
        .map_err(|err| anyhow!("failed to query synchronizer at {endpoint}: {err}"))?;
    if !response.status().is_success() {
        return Err(anyhow!(
            "synchronizer request failed: {} {}",
            response.status().as_u16(),
            response.status()
        ));
    }

    let payload: MembershipResponse = response
        .json()
        .map_err(|err| anyhow!("failed to decode synchronizer membership response: {err}"))?;

    let mut grounded_txs = HashSet::new();
    for entry in payload.tx_results {
        if entry.present {
            grounded_txs.insert(parse_hash_hex(&entry.tx_hash)?);
        }
    }

    let mut on_chain_nullifiers = HashSet::new();
    for entry in payload.nullifier_results {
        if entry.present {
            on_chain_nullifiers.insert(parse_hash_hex(&entry.nullifier)?);
        }
    }

    Ok(SynchronizerMembership {
        grounded_txs,
        on_chain_nullifiers,
    })
}

fn fetch_synchronizer_tx_status(sync_api_url: &str, tx_hash: &Hash) -> Result<TxStatusResponse> {
    let endpoint = format!(
        "{}/v1/state/tx/{}",
        sync_api_url.trim_end_matches('/'),
        encode_hash_hex(tx_hash)
    );
    let response = reqwest::blocking::get(&endpoint)
        .map_err(|err| anyhow!("failed to query synchronizer at {endpoint}: {err}"))?;
    if !response.status().is_success() {
        return Err(anyhow!(
            "synchronizer request failed: {} {}",
            response.status().as_u16(),
            response.status()
        ));
    }

    response
        .json::<TxStatusResponse>()
        .map_err(|err| anyhow!("failed to decode synchronizer tx status response: {err}"))
}

pub(crate) fn wait_for_synchronizer_tx(
    sync_api_url: &str,
    tx_final: Hash,
    timeout_secs: u64,
    poll_interval_ms: u64,
) -> Result<SynchronizerHead> {
    let timeout = Duration::from_secs(timeout_secs);
    let poll_interval = Duration::from_millis(poll_interval_ms);
    let start = Instant::now();
    loop {
        let status = fetch_synchronizer_tx_status(sync_api_url, &tx_final)?;
        if status.present {
            return fetch_synchronizer_head(sync_api_url);
        }
        if start.elapsed() >= timeout {
            return Err(anyhow!(
                "synchronizer did not index relayed tx {} within {}s",
                encode_hash_hex(&tx_final),
                timeout_secs
            ));
        }
        std::thread::sleep(poll_interval);
    }
}
