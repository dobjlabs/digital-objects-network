use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow};
use pod2::middleware::Hash;
use serde::de::DeserializeOwned;
use txlib::{GroundingWitness, StateHeader};
use wire_types::synchronizer::{
    GroundingWitnessRequest, GroundingWitnessResponse, MembershipRequest, MembershipResponse,
    StateHeadResponse,
};

pub const SYNCHRONIZER_POLL_TIMEOUT_SECS: u64 = 120;
pub const SYNCHRONIZER_POLL_INTERVAL_MS: u64 = 1200;

/// Per-request cap on `tx_hashes.len() + nullifiers.len()` accepted by the
/// synchronizer's `POST /v1/state/membership` endpoint. MUST stay at or
/// below the server's `MAX_HASH_QUERY_ITEMS` (synchronizer/src/api.rs).
/// Above that, the server returns 400 and `sync_objects` falls back to
/// stale local listing without on-chain reconciliation.
///
/// We chunk client-side at this value so inventories with hundreds of
/// objects still reconcile in a single `sync_objects` call (just spread
/// across multiple HTTP requests).
const HASH_BATCH_LIMIT: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SynchronizerHead {
    pub current_state_root: Hash,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SynchronizerMembership {
    /// Object commitments present in the global created set.
    pub created_objects: HashSet<Hash>,
    pub on_chain_nullifiers: HashSet<Hash>,
}

pub trait SynchronizerClient: Send + Sync {
    fn fetch_head(&self, sync_api_url: &str) -> Result<SynchronizerHead>;
    fn fetch_grounding_witness(
        &self,
        sync_api_url: &str,
        object_commitments: &[Hash],
    ) -> Result<GroundingWitness>;
    fn fetch_membership_with_nullifiers(
        &self,
        sync_api_url: &str,
        object_commitments: &[Hash],
        nullifiers: &[Hash],
    ) -> Result<SynchronizerMembership>;
    /// Wait until a transaction has landed: every object commitment it produces
    /// is in the created set and every nullifier it emits is in the
    /// nullifier set. Returns the resulting head once the whole union
    /// is present, or errors on timeout.
    fn wait_for_tx(
        &self,
        sync_api_url: &str,
        created_commitments: &[Hash],
        nullifiers: &[Hash],
        timeout_secs: u64,
        poll_interval_ms: u64,
    ) -> Result<SynchronizerHead>;
}

#[derive(Debug, Clone)]
pub struct HttpSynchronizerClient {
    client: reqwest::blocking::Client,
}

impl Default for HttpSynchronizerClient {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpSynchronizerClient {
    pub fn new() -> Self {
        Self {
            client: super::build_http_client(),
        }
    }
}

impl SynchronizerClient for HttpSynchronizerClient {
    fn fetch_head(&self, sync_api_url: &str) -> Result<SynchronizerHead> {
        let endpoint = format!("{}/v1/state/head", sync_api_url.trim_end_matches('/'));
        let payload: StateHeadResponse = send_json_request(
            self.client.get(&endpoint),
            &endpoint,
            "synchronizer head response",
        )?;
        let current_state_root = payload
            .current_state_root
            .ok_or_else(|| anyhow!("synchronizer has no grounded state yet"))?;
        Ok(SynchronizerHead { current_state_root })
    }

    fn fetch_grounding_witness(
        &self,
        sync_api_url: &str,
        object_commitments: &[Hash],
    ) -> Result<GroundingWitness> {
        let endpoint = format!(
            "{}/v1/txlib/grounding-witness",
            sync_api_url.trim_end_matches('/')
        );
        let request = GroundingWitnessRequest {
            object_commitments: object_commitments.to_vec(),
        };
        let payload: GroundingWitnessResponse = send_json_request(
            self.client.post(&endpoint).json(&request),
            &endpoint,
            "synchronizer grounding witness response",
        )?;

        let state_header = StateHeader::new(
            payload.block_number,
            payload.created_root,
            payload.nullifiers_root,
            payload.prior_state_history_root,
        );
        let remote_state_root = payload.state_root;
        let derived_state_root = state_header.hash();
        if remote_state_root != derived_state_root {
            return Err(anyhow!(
                "synchronizer grounding witness hash mismatch: remote={remote_state_root:#} derived={derived_state_root:#}"
            ));
        }

        let created_proofs = collect_created_proofs(
            object_commitments,
            payload.created_proofs.into_iter().map(|entry| {
                (
                    entry.commitment,
                    entry.present,
                    entry.index.zip(entry.proof),
                )
            }),
        )?;

        Ok(GroundingWitness::new(state_header, created_proofs))
    }

    fn fetch_membership_with_nullifiers(
        &self,
        sync_api_url: &str,
        object_commitments: &[Hash],
        nullifiers: &[Hash],
    ) -> Result<SynchronizerMembership> {
        let endpoint = format!("{}/v1/state/membership", sync_api_url.trim_end_matches('/'));

        let mut created_objects: HashSet<Hash> = HashSet::new();
        let mut on_chain_nullifiers: HashSet<Hash> = HashSet::new();

        // Empty inputs need no network call: the membership endpoint accepts a
        // request with both lists empty, but it is a wasted round trip.
        if object_commitments.is_empty() && nullifiers.is_empty() {
            return Ok(SynchronizerMembership {
                created_objects,
                on_chain_nullifiers,
            });
        }

        // The synchronizer rejects bodies where `object_commitments.len() +
        // nullifiers.len() > HASH_BATCH_LIMIT` with 400. Pack each
        // batch tightly: take as many object commitments as possible (up to
        // the limit), then fill the remainder with nullifiers. Continue until
        // both lists are drained. Sequential because reqwest::blocking
        // and synchronizer responses are fast.
        let mut obj_cursor = 0usize;
        let mut null_cursor = 0usize;
        while obj_cursor < object_commitments.len() || null_cursor < nullifiers.len() {
            let obj_take = (object_commitments.len() - obj_cursor).min(HASH_BATCH_LIMIT);
            let remaining_capacity = HASH_BATCH_LIMIT - obj_take;
            let null_take = (nullifiers.len() - null_cursor).min(remaining_capacity);

            let request = MembershipRequest {
                object_commitments: object_commitments[obj_cursor..obj_cursor + obj_take].to_vec(),
                nullifiers: nullifiers[null_cursor..null_cursor + null_take].to_vec(),
            };

            let payload: MembershipResponse = send_json_request(
                self.client.post(&endpoint).json(&request),
                &endpoint,
                "synchronizer membership response",
            )?;

            for entry in payload.created_results {
                if entry.present {
                    created_objects.insert(entry.commitment);
                }
            }
            for entry in payload.nullifier_results {
                if entry.present {
                    on_chain_nullifiers.insert(entry.nullifier);
                }
            }

            obj_cursor += obj_take;
            null_cursor += null_take;
        }

        Ok(SynchronizerMembership {
            created_objects,
            on_chain_nullifiers,
        })
    }

    fn wait_for_tx(
        &self,
        sync_api_url: &str,
        created_commitments: &[Hash],
        nullifiers: &[Hash],
        timeout_secs: u64,
        poll_interval_ms: u64,
    ) -> Result<SynchronizerHead> {
        let timeout = Duration::from_secs(timeout_secs);
        let poll_interval = Duration::from_millis(poll_interval_ms);
        let start = Instant::now();
        loop {
            let membership = match self.fetch_membership_with_nullifiers(
                sync_api_url,
                created_commitments,
                nullifiers,
            ) {
                Ok(membership) => membership,
                Err(err) if super::is_retryable_request_error(&err) => {
                    if start.elapsed() >= timeout {
                        return Err(anyhow!(
                            "synchronizer did not observe the transaction within {}s; last membership query failed: {err:#}",
                            timeout_secs
                        ));
                    }
                    std::thread::sleep(poll_interval);
                    continue;
                }
                Err(err) => return Err(err),
            };
            let landed = created_commitments
                .iter()
                .all(|c| membership.created_objects.contains(c))
                && nullifiers
                    .iter()
                    .all(|n| membership.on_chain_nullifiers.contains(n));
            if landed {
                match self.fetch_head(sync_api_url) {
                    Ok(head) => return Ok(head),
                    Err(err) if super::is_retryable_request_error(&err) => {
                        if start.elapsed() >= timeout {
                            return Err(anyhow!(
                                "synchronizer observed the transaction but did not return a head within {}s; last head query failed: {err:#}",
                                timeout_secs
                            ));
                        }
                        std::thread::sleep(poll_interval);
                        continue;
                    }
                    Err(err) => return Err(err),
                }
            }
            if start.elapsed() >= timeout {
                return Err(anyhow!(
                    "synchronizer did not observe the transaction within {}s",
                    timeout_secs
                ));
            }
            std::thread::sleep(poll_interval);
        }
    }
}

fn send_json_request<T: DeserializeOwned>(
    request: reqwest::blocking::RequestBuilder,
    endpoint: &str,
    decode_context: &str,
) -> Result<T> {
    let response = request
        .send()
        .with_context(|| format!("failed to query endpoint at {endpoint}"))?;
    if !response.status().is_success() {
        return Err(anyhow!(
            "request failed with {} {}",
            response.status().as_u16(),
            response.status()
        ));
    }

    response
        .json()
        .with_context(|| format!("failed to decode {decode_context}"))
}

/// Validate and index the per-object created-set proofs returned by the
/// synchronizer against the commitments we requested. Rejects unexpected,
/// conflicting, omitted, or not-yet-present entries.
fn collect_created_proofs<P>(
    requested_commitments: &[Hash],
    entries: impl IntoIterator<Item = (Hash, bool, Option<P>)>,
) -> Result<HashMap<Hash, P>> {
    let expected_hashes = requested_commitments
        .iter()
        .copied()
        .collect::<HashSet<_>>();
    let mut response_presence = HashMap::new();
    let mut created_proofs = HashMap::new();

    for (commitment, present, proof) in entries {
        if !expected_hashes.contains(&commitment) {
            return Err(anyhow!(
                "synchronizer grounding witness response contained unexpected object proof: {commitment:#}"
            ));
        }

        if let Some(previous_present) = response_presence.insert(commitment, present)
            && previous_present != present
        {
            return Err(anyhow!(
                "synchronizer grounding witness response contained conflicting entries for object {commitment:#}"
            ));
        }

        if present {
            let proof = proof.ok_or_else(|| {
                anyhow!(
                    "synchronizer reported object {commitment:#} present but omitted its array proof"
                )
            })?;
            created_proofs.insert(commitment, proof);
        }
    }

    let omitted = render_requested_hashes(requested_commitments, |commitment| {
        !response_presence.contains_key(commitment)
    });
    if !omitted.is_empty() {
        return Err(anyhow!(
            "synchronizer grounding witness response omitted requested object proofs: {}",
            omitted.join(", ")
        ));
    }

    let unavailable = render_requested_hashes(requested_commitments, |commitment| {
        response_presence
            .get(commitment)
            .is_some_and(|present| !*present)
    });
    if !unavailable.is_empty() {
        return Err(anyhow!(
            "input not yet synchronized; wait and retry: {}",
            unavailable.join(", ")
        ));
    }

    Ok(created_proofs)
}

fn render_requested_hashes(
    requested_commitments: &[Hash],
    include: impl Fn(&Hash) -> bool,
) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut rendered = Vec::new();
    for commitment in requested_commitments {
        if seen.insert(*commitment) && include(commitment) {
            rendered.push(format!("{commitment:#}"));
        }
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::*;
    use pod2::middleware::RawValue;

    fn test_hash(byte: u8) -> Hash {
        Hash::from(RawValue::from(i64::from(byte)))
    }

    #[test]
    fn collect_created_proofs_rejects_omitted_requested_hash() {
        let requested = [test_hash(1), test_hash(2)];
        let proofs = vec![(requested[0], true, Some("proof-1"))];

        let err = collect_created_proofs(&requested, proofs).expect_err("should fail");

        assert!(
            err.to_string().contains(&format!("{:#}", requested[1])),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn collect_created_proofs_rejects_unexpected_hash() {
        let requested = [test_hash(1)];
        let unexpected = test_hash(9);
        let proofs = vec![(unexpected, true, Some("proof-9"))];

        let err = collect_created_proofs(&requested, proofs).expect_err("should fail");

        assert!(
            err.to_string().contains(&format!("{:#}", unexpected)),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn collect_created_proofs_rejects_conflicting_duplicate_status() {
        let requested = [test_hash(1)];
        let proofs = vec![
            (requested[0], true, Some("proof-1a")),
            (requested[0], false, None),
        ];

        let err = collect_created_proofs(&requested, proofs).expect_err("should fail");

        assert!(
            err.to_string().contains("conflicting entries"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn collect_created_proofs_allows_duplicate_requested_hashes() {
        let requested = [test_hash(1), test_hash(1), test_hash(2)];
        let proofs = vec![
            (requested[0], true, Some("proof-1")),
            (requested[2], true, Some("proof-2")),
        ];

        let result = collect_created_proofs(&requested, proofs).expect("should succeed");

        assert_eq!(result.len(), 2);
        assert_eq!(result.get(&requested[0]), Some(&"proof-1"));
        assert_eq!(result.get(&requested[2]), Some(&"proof-2"));
    }
}
