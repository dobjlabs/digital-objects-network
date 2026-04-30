//! Blocking HTTP clients for the synchronizer + relayer.
//!
//! These intentionally use `reqwest::blocking` so the driver stays a simple
//! synchronous library. GUI hosts (Tauri) can wrap calls in `spawn_blocking`.
//!
//! Grounding witness shape uses the new SHA-256 SMT `MerkleProof` from
//! `txlib_core::merkle` — JSON-compatible with the synchronizer's
//! `/v1/txlib/grounding-witness` endpoint.

use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use txlib_core::Hash;
use txlib_core::merkle::MerkleProof;

pub const SYNCHRONIZER_POLL_INTERVAL_MS: u64 = 1_000;
pub const SYNCHRONIZER_POLL_TIMEOUT_SECS: u64 = 600;
pub const RELAYER_POLL_INTERVAL_MS: u64 = 2_000;
pub const RELAYER_POLL_TIMEOUT_SECS: u64 = 300;

// ===========================================================================
// Synchronizer
// ===========================================================================

#[derive(Debug, Clone, Deserialize)]
struct StateHeadResponse {
    last_processed_slot: u32,
    #[serde(default)]
    current_gsr: Option<String>,
    #[serde(default)]
    tx_count: u64,
    #[serde(default)]
    nullifier_count: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GroundingWitnessRequest<'a> {
    source_tx_hashes: Vec<&'a str>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GroundingWitnessResponse {
    state_root_hash: String,
    block_number: i64,
    transactions_root: String,
    nullifiers_root: String,
    gsrs_root: String,
    source_tx_proofs: Vec<SourceTxProofResponse>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SourceTxProofResponse {
    tx_hash: String,
    present: bool,
    proof: MerkleProof,
}

#[derive(Debug, Clone)]
pub struct SyncStateHead {
    pub last_processed_slot: u32,
    pub current_gsr: Option<Hash>,
    pub tx_count: u64,
    pub nullifier_count: u64,
}

/// One source-tx grounding witness pulled from the synchronizer.
#[derive(Debug, Clone)]
pub struct SourceTxWitness {
    pub source_tx_final: Hash,
    pub present: bool,
    pub tx_inclusion_proof: MerkleProof,
}

#[derive(Debug, Clone)]
pub struct GroundingWitness {
    pub state_root_hash: Hash,
    pub block_number: i64,
    pub transactions_root: Hash,
    pub nullifiers_root: Hash,
    pub gsrs_root: Hash,
    pub witnesses: Vec<SourceTxWitness>,
}

pub trait SynchronizerClient: Send + Sync {
    fn state_head(&self) -> Result<SyncStateHead>;
    fn grounding_witness(&self, source_tx_hashes: &[Hash]) -> Result<GroundingWitness>;
    fn tx_present(&self, tx_final: Hash) -> Result<bool>;
}

/// HTTP-backed implementation talking to the synchronizer's REST API.
pub struct HttpSynchronizerClient {
    base_url: String,
    http: Client,
}

impl HttpSynchronizerClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        let http = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("default reqwest client");
        Self {
            base_url: base_url.into(),
            http,
        }
    }
}

impl SynchronizerClient for HttpSynchronizerClient {
    fn state_head(&self) -> Result<SyncStateHead> {
        let url = format!("{}/v1/state/head", self.base_url.trim_end_matches('/'));
        let resp: StateHeadResponse = self
            .http
            .get(&url)
            .send()
            .with_context(|| format!("GET {url}"))?
            .error_for_status()
            .with_context(|| format!("GET {url} status"))?
            .json()
            .with_context(|| format!("GET {url} body"))?;
        Ok(SyncStateHead {
            last_processed_slot: resp.last_processed_slot,
            current_gsr: resp
                .current_gsr
                .map(|s| parse_hash_hex(&s))
                .transpose()?,
            tx_count: resp.tx_count,
            nullifier_count: resp.nullifier_count,
        })
    }

    fn grounding_witness(&self, source_tx_hashes: &[Hash]) -> Result<GroundingWitness> {
        let hex_strings: Vec<String> = source_tx_hashes
            .iter()
            .map(|h| format!("{h}"))
            .collect();
        let req = GroundingWitnessRequest {
            source_tx_hashes: hex_strings.iter().map(|s| s.as_str()).collect(),
        };
        let url = format!(
            "{}/v1/txlib/grounding-witness",
            self.base_url.trim_end_matches('/')
        );
        let resp: GroundingWitnessResponse = self
            .http
            .post(&url)
            .json(&req)
            .send()
            .with_context(|| format!("POST {url}"))?
            .error_for_status()
            .with_context(|| format!("POST {url} status"))?
            .json()
            .with_context(|| format!("POST {url} body"))?;
        Ok(GroundingWitness {
            state_root_hash: parse_hash_hex(&resp.state_root_hash)?,
            block_number: resp.block_number,
            transactions_root: parse_hash_hex(&resp.transactions_root)?,
            nullifiers_root: parse_hash_hex(&resp.nullifiers_root)?,
            gsrs_root: parse_hash_hex(&resp.gsrs_root)?,
            witnesses: resp
                .source_tx_proofs
                .into_iter()
                .map(|p| {
                    Ok(SourceTxWitness {
                        source_tx_final: parse_hash_hex(&p.tx_hash)?,
                        present: p.present,
                        tx_inclusion_proof: p.proof,
                    })
                })
                .collect::<Result<Vec<_>>>()?,
        })
    }

    fn tx_present(&self, tx_final: Hash) -> Result<bool> {
        #[derive(Deserialize)]
        struct R {
            present: bool,
        }
        let url = format!(
            "{}/v1/state/tx/{}",
            self.base_url.trim_end_matches('/'),
            tx_final
        );
        let resp: R = self
            .http
            .get(&url)
            .send()
            .with_context(|| format!("GET {url}"))?
            .error_for_status()
            .with_context(|| format!("GET {url} status"))?
            .json()
            .with_context(|| format!("GET {url} body"))?;
        Ok(resp.present)
    }
}

// ===========================================================================
// Relayer
// ===========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum JobStatus {
    Queued,
    Sending,
    Submitted,
    Confirmed,
    Failed,
}

impl JobStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, JobStatus::Confirmed | JobStatus::Failed)
    }
}

#[derive(Debug, Clone)]
pub struct RelaySubmission {
    pub job_id: String,
    pub status: JobStatus,
    pub tx_final: Hash,
    pub state_root_hash: Hash,
}

#[derive(Debug, Clone)]
pub struct RelayJobStatus {
    pub job_id: String,
    pub status: JobStatus,
    pub tx_hash: Option<String>,
    pub block_number: Option<u64>,
    pub last_error: Option<String>,
    pub tx_final: Hash,
}

pub trait RelayerClient: Send + Sync {
    /// Submit a blob payload (the exact bytes a synchronizer would parse —
    /// `magic | bincode-Receipt`).
    fn submit_payload(
        &self,
        payload_bytes: &[u8],
        client_ref: Option<&str>,
    ) -> Result<RelaySubmission>;
    fn job_status(&self, job_id: &str) -> Result<RelayJobStatus>;
}

pub struct HttpRelayerClient {
    base_url: String,
    http: Client,
}

impl HttpRelayerClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        let http = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("default reqwest client");
        Self {
            base_url: base_url.into(),
            http,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct SubmitProofRequest<'a> {
    payload_base64: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    client_ref: Option<&'a str>,
}

#[derive(Debug, Clone, Deserialize)]
struct SubmitProofResponse {
    job_id: String,
    status: JobStatus,
    tx_final: String,
    state_root_hash: String,
}

#[derive(Debug, Clone, Deserialize)]
struct JobStatusResponse {
    job_id: String,
    status: JobStatus,
    #[serde(default)]
    tx_hash: Option<String>,
    #[serde(default)]
    block_number: Option<u64>,
    #[serde(default)]
    last_error: Option<String>,
    tx_final: String,
}

impl RelayerClient for HttpRelayerClient {
    fn submit_payload(
        &self,
        payload_bytes: &[u8],
        client_ref: Option<&str>,
    ) -> Result<RelaySubmission> {
        let url = format!(
            "{}/api/v1/proofs",
            self.base_url.trim_end_matches('/')
        );
        let body = SubmitProofRequest {
            payload_base64: B64.encode(payload_bytes),
            client_ref,
        };
        let resp: SubmitProofResponse = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .with_context(|| format!("POST {url}"))?
            .error_for_status()
            .with_context(|| format!("POST {url} status"))?
            .json()
            .with_context(|| format!("POST {url} body"))?;
        Ok(RelaySubmission {
            job_id: resp.job_id,
            status: resp.status,
            tx_final: parse_hash_hex(&resp.tx_final)?,
            state_root_hash: parse_hash_hex(&resp.state_root_hash)?,
        })
    }

    fn job_status(&self, job_id: &str) -> Result<RelayJobStatus> {
        let url = format!(
            "{}/api/v1/proofs/{}",
            self.base_url.trim_end_matches('/'),
            job_id
        );
        let resp: JobStatusResponse = self
            .http
            .get(&url)
            .send()
            .with_context(|| format!("GET {url}"))?
            .error_for_status()
            .with_context(|| format!("GET {url} status"))?
            .json()
            .with_context(|| format!("GET {url} body"))?;
        Ok(RelayJobStatus {
            job_id: resp.job_id,
            status: resp.status,
            tx_hash: resp.tx_hash,
            block_number: resp.block_number,
            last_error: resp.last_error,
            tx_final: parse_hash_hex(&resp.tx_final)?,
        })
    }
}

// ===========================================================================
// Helpers
// ===========================================================================

fn parse_hash_hex(value: &str) -> Result<Hash> {
    let trimmed = value.trim().strip_prefix("0x").unwrap_or(value.trim());
    if trimmed.len() != 64 {
        return Err(anyhow!(
            "invalid hash `{value}`: expected 64 hex chars, got {}",
            trimmed.len()
        ));
    }
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = u8::from_str_radix(&trimmed[2 * i..2 * i + 2], 16)
            .map_err(|e| anyhow!("invalid hash `{value}`: {e}"))?;
    }
    Ok(Hash(out))
}
