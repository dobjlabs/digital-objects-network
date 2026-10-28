use std::collections::{HashMap, HashSet};

use super::send_json_request;
use anyhow::{Result, anyhow};
use pod2::middleware::Hash;
use txlib::{GroundingWitness, StateHeader};
use wire_types::synchronizer::{
    GroundingWitnessRequest, GroundingWitnessResponse, MembershipRequest, MembershipResponse,
    StateHeadResponse,
};

/// Combined commitment and nullifier limit for each membership request.
/// Must not exceed the synchronizer's request limit.
const HASH_BATCH_LIMIT: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SynchronizerHead {
    pub state_root: Hash,
    pub block_number: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SynchronizerMembership {
    /// Object commitments present in the global created set.
    pub created_objects: HashSet<Hash>,
    pub on_chain_nullifiers: HashSet<Hash>,
    /// Oldest head block used across the response's batches. Absence at
    /// this block implies expiry only when the grounding is already too old.
    pub block_number: i64,
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
        Ok(SynchronizerHead {
            state_root: payload.state_root,
            block_number: payload.block_number,
        })
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
            payload.block_timestamp,
            payload.block_hash,
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
        let mut block_number: Option<i64> = None;

        // An empty query still needs one request to read the head.
        let mut obj_cursor = 0usize;
        let mut null_cursor = 0usize;
        loop {
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
            block_number = Some(match block_number {
                Some(earliest) => earliest.min(payload.block_number),
                None => payload.block_number,
            });

            obj_cursor += obj_take;
            null_cursor += null_take;
            if obj_cursor >= object_commitments.len() && null_cursor >= nullifiers.len() {
                break;
            }
        }

        Ok(SynchronizerMembership {
            created_objects,
            on_chain_nullifiers,
            block_number: block_number.expect("at least one membership request was made"),
        })
    }
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
