use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use anyhow::{Result, anyhow};
use pod2::middleware::Hash;
use serde::de::DeserializeOwned;
use synchronizer::api_types::{
    GroundingWitnessRequest, GroundingWitnessResponse, MembershipRequest, MembershipResponse,
    StateHeadResponse, TxStatusResponse,
};
use txlib::{GroundingWitness, StateRoot};

use common::{decode_hash_hex, encode_hash_hex};

pub const SYNCHRONIZER_POLL_TIMEOUT_SECS: u64 = 120;
pub const SYNCHRONIZER_POLL_INTERVAL_MS: u64 = 1200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SynchronizerHead {
    pub current_gsr: Hash,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SynchronizerMembership {
    pub grounded_txs: HashSet<Hash>,
    pub on_chain_nullifiers: HashSet<Hash>,
}

pub trait SynchronizerClient: Send + Sync {
    fn fetch_head(&self, sync_api_url: &str) -> Result<SynchronizerHead>;
    fn fetch_grounding_witness(
        &self,
        sync_api_url: &str,
        source_tx_hashes: &[Hash],
    ) -> Result<GroundingWitness>;
    fn fetch_membership_with_nullifiers(
        &self,
        sync_api_url: &str,
        tx_hashes: &[Hash],
        nullifiers: &[Hash],
    ) -> Result<SynchronizerMembership>;
    fn wait_for_tx(
        &self,
        sync_api_url: &str,
        tx_final: Hash,
        timeout_secs: u64,
        poll_interval_ms: u64,
    ) -> Result<SynchronizerHead>;
}

#[derive(Debug, Default)]
pub struct HttpSynchronizerClient;

impl SynchronizerClient for HttpSynchronizerClient {
    fn fetch_head(&self, sync_api_url: &str) -> Result<SynchronizerHead> {
        let endpoint = format!("{}/v1/state/head", sync_api_url.trim_end_matches('/'));
        let client = reqwest::blocking::Client::new();
        let payload: StateHeadResponse = send_json_request(
            client.get(&endpoint),
            &endpoint,
            "synchronizer head response",
        )?;
        let current_gsr = payload
            .current_gsr
            .as_deref()
            .ok_or_else(|| anyhow!("synchronizer has no canonical grounded state yet"))
            .and_then(parse_hash_hex)?;
        Ok(SynchronizerHead { current_gsr })
    }

    fn fetch_grounding_witness(
        &self,
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
        let payload: GroundingWitnessResponse = send_json_request(
            client.post(&endpoint).json(&request),
            &endpoint,
            "synchronizer grounding witness response",
        )?;

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

        let source_tx_proofs = collect_source_tx_proofs(
            source_tx_hashes,
            payload
                .source_tx_proofs
                .into_iter()
                .map(|entry| (entry.tx_hash, entry.present, entry.proof)),
        )?;

        Ok(GroundingWitness::new(state_root, source_tx_proofs))
    }

    fn fetch_membership_with_nullifiers(
        &self,
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
        let payload: MembershipResponse = send_json_request(
            client.post(&endpoint).json(&request),
            &endpoint,
            "synchronizer membership response",
        )?;

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

    fn wait_for_tx(
        &self,
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
                return self.fetch_head(sync_api_url);
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
}

fn parse_hash_hex(value: &str) -> Result<Hash> {
    decode_hash_hex(value.trim())
}

fn send_json_request<T: DeserializeOwned>(
    request: reqwest::blocking::RequestBuilder,
    endpoint: &str,
    decode_context: &str,
) -> Result<T> {
    let response = request
        .send()
        .map_err(|err| anyhow!("failed to query endpoint at {endpoint}: {err}"))?;
    if !response.status().is_success() {
        return Err(anyhow!(
            "request failed with {} {}",
            response.status().as_u16(),
            response.status()
        ));
    }

    response
        .json()
        .map_err(|err| anyhow!("failed to decode {decode_context}: {err}"))
}

fn collect_source_tx_proofs<P>(
    requested_source_tx_hashes: &[Hash],
    entries: impl IntoIterator<Item = (String, bool, P)>,
) -> Result<HashMap<Hash, P>> {
    let expected_hashes = requested_source_tx_hashes
        .iter()
        .copied()
        .collect::<HashSet<_>>();
    let mut response_presence = HashMap::new();
    let mut source_tx_proofs = HashMap::new();

    for (tx_hash_raw, present, proof) in entries {
        let tx_hash = parse_hash_hex(&tx_hash_raw)?;
        if !expected_hashes.contains(&tx_hash) {
            return Err(anyhow!(
                "synchronizer grounding witness response contained unexpected source tx proof: {}",
                encode_hash_hex(&tx_hash)
            ));
        }

        if let Some(previous_present) = response_presence.insert(tx_hash, present)
            && previous_present != present
        {
            return Err(anyhow!(
                "synchronizer grounding witness response contained conflicting entries for source tx {}",
                encode_hash_hex(&tx_hash)
            ));
        }

        if present {
            source_tx_proofs.insert(tx_hash, proof);
        }
    }

    let omitted = render_requested_hashes(requested_source_tx_hashes, |tx_hash| {
        !response_presence.contains_key(tx_hash)
    });
    if !omitted.is_empty() {
        return Err(anyhow!(
            "synchronizer grounding witness response omitted requested source tx proofs: {}",
            omitted.join(", ")
        ));
    }

    let unavailable = render_requested_hashes(requested_source_tx_hashes, |tx_hash| {
        response_presence
            .get(tx_hash)
            .is_some_and(|present| !*present)
    });
    if !unavailable.is_empty() {
        return Err(anyhow!(
            "input not yet synchronized; wait and retry: {}",
            unavailable.join(", ")
        ));
    }

    Ok(source_tx_proofs)
}

fn render_requested_hashes(
    requested_source_tx_hashes: &[Hash],
    include: impl Fn(&Hash) -> bool,
) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut rendered = Vec::new();
    for tx_hash in requested_source_tx_hashes {
        if seen.insert(*tx_hash) && include(tx_hash) {
            rendered.push(encode_hash_hex(tx_hash));
        }
    }
    rendered
}

fn fetch_synchronizer_tx_status(sync_api_url: &str, tx_hash: &Hash) -> Result<TxStatusResponse> {
    let endpoint = format!(
        "{}/v1/state/tx/{}",
        sync_api_url.trim_end_matches('/'),
        encode_hash_hex(tx_hash)
    );
    let client = reqwest::blocking::Client::new();
    send_json_request(
        client.get(&endpoint),
        &endpoint,
        "synchronizer tx status response",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_hash(byte: u8) -> Hash {
        decode_hash_hex(&hex::encode([byte; 32])).expect("valid test hash")
    }

    #[test]
    fn collect_source_tx_proofs_rejects_omitted_requested_hash() {
        let requested = [test_hash(1), test_hash(2)];
        let proofs = vec![(encode_hash_hex(&requested[0]), true, "proof-1")];

        let err = collect_source_tx_proofs(&requested, proofs).expect_err("should fail");

        assert!(
            err.to_string().contains(&encode_hash_hex(&requested[1])),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn collect_source_tx_proofs_rejects_unexpected_hash() {
        let requested = [test_hash(1)];
        let unexpected = test_hash(9);
        let proofs = vec![(encode_hash_hex(&unexpected), true, "proof-9")];

        let err = collect_source_tx_proofs(&requested, proofs).expect_err("should fail");

        assert!(
            err.to_string().contains(&encode_hash_hex(&unexpected)),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn collect_source_tx_proofs_rejects_conflicting_duplicate_status() {
        let requested = [test_hash(1)];
        let proofs = vec![
            (encode_hash_hex(&requested[0]), true, "proof-1a"),
            (encode_hash_hex(&requested[0]), false, "proof-1b"),
        ];

        let err = collect_source_tx_proofs(&requested, proofs).expect_err("should fail");

        assert!(
            err.to_string().contains("conflicting entries"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn collect_source_tx_proofs_allows_duplicate_requested_hashes() {
        let requested = [test_hash(1), test_hash(1), test_hash(2)];
        let proofs = vec![
            (encode_hash_hex(&requested[0]), true, "proof-1"),
            (encode_hash_hex(&requested[2]), true, "proof-2"),
        ];

        let result = collect_source_tx_proofs(&requested, proofs).expect("should succeed");

        assert_eq!(result.len(), 2);
        assert_eq!(result.get(&requested[0]), Some(&"proof-1"));
        assert_eq!(result.get(&requested[2]), Some(&"proof-2"));
    }
}
