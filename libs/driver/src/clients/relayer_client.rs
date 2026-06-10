use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use base64::{Engine, engine::general_purpose::STANDARD};
use payload::blob::MAX_SIMPLE_BLOB_PAYLOAD_BYTES;
use pod2::middleware::Hash;
use wire_types::relayer::{
    JobStatus, JobStatusResponse, SubmitProofRequest, SubmitProofResponse,
    TxHashesByTxFinalRequest, TxHashesByTxFinalResponse,
};

/// Deadline for the relayer to broadcast the tx and expose a tx_hash.
pub const RELAYER_TX_HASH_TIMEOUT_SECS: u64 = 180;
/// Deadline for the broadcast tx to reach `Confirmed`. Tracked separately
/// from the tx-hash wait so the two sequential phases each have an explicit
/// budget rather than silently sharing (and doubling) one constant.
pub const RELAYER_CONFIRM_TIMEOUT_SECS: u64 = 180;
pub const RELAYER_POLL_INTERVAL_MS: u64 = 1500;

/// Per-request cap on `tx_finals` sent to the relayer's `tx-hashes` endpoint.
/// MUST stay at or below the server's `MAX_TX_HASH_QUERY_ITEMS`.
const TX_HASH_BATCH_LIMIT: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayerConfirmation {
    pub job_id: String,
    pub tx_hash: Option<String>,
    pub block_number: Option<i64>,
}

pub trait RelayerClient: Send + Sync {
    fn submit_proof(
        &self,
        relayer_api_url: &str,
        payload_bytes: &[u8],
        client_ref: Option<String>,
    ) -> Result<SubmitProofResponse>;
    /// Poll until the relayer has broadcast the transaction and a tx_hash is available.
    fn wait_for_tx_hash(
        &self,
        relayer_api_url: &str,
        job_id: &str,
        timeout_secs: u64,
        poll_interval_ms: u64,
    ) -> Result<String>;
    fn wait_for_confirmation(
        &self,
        relayer_api_url: &str,
        job_id: &str,
        timeout_secs: u64,
        poll_interval_ms: u64,
    ) -> Result<RelayerConfirmation>;
    /// Resolve the current Ethereum tx hash for each given `tx_final` (proof
    /// commitment), in one batched call. Commitments the relayer has no
    /// broadcast hash for are omitted from the returned map.
    fn lookup_tx_hashes(
        &self,
        relayer_api_url: &str,
        tx_finals: &[Hash],
    ) -> Result<HashMap<Hash, String>>;
}

#[derive(Debug, Clone)]
pub struct HttpRelayerClient {
    client: reqwest::blocking::Client,
}

impl Default for HttpRelayerClient {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpRelayerClient {
    pub fn new() -> Self {
        Self {
            client: super::build_http_client(),
        }
    }

    fn fetch_job_status(&self, relayer_api_url: &str, job_id: &str) -> Result<JobStatusResponse> {
        let endpoint = format!(
            "{}/api/v1/proofs/{job_id}",
            relayer_api_url.trim_end_matches('/')
        );
        let response = self
            .client
            .get(&endpoint)
            .send()
            .with_context(|| format!("failed to query relayer job at {endpoint}"))?;

        let status = response.status();
        let body = response
            .text()
            .with_context(|| format!("failed to read relayer status response from {endpoint}"))?;
        if !status.is_success() {
            return Err(anyhow!(
                "relayer status failed with {} {}: {}",
                status.as_u16(),
                status,
                body
            ));
        }

        serde_json::from_str::<JobStatusResponse>(&body)
            .map_err(|err| anyhow!("failed to decode relayer status response: {err}; body={body}"))
    }
}

impl RelayerClient for HttpRelayerClient {
    fn submit_proof(
        &self,
        relayer_api_url: &str,
        payload_bytes: &[u8],
        client_ref: Option<String>,
    ) -> Result<SubmitProofResponse> {
        if payload_bytes.len() > MAX_SIMPLE_BLOB_PAYLOAD_BYTES {
            return Err(anyhow!(
                "payload exceeds single-blob limit: {} > {}",
                payload_bytes.len(),
                MAX_SIMPLE_BLOB_PAYLOAD_BYTES
            ));
        }

        let endpoint = format!("{}/api/v1/proofs", relayer_api_url.trim_end_matches('/'));
        let request = SubmitProofRequest {
            payload_base64: STANDARD.encode(payload_bytes),
            client_ref,
        };

        let response = self
            .client
            .post(&endpoint)
            .json(&request)
            .send()
            .map_err(|err| anyhow!("failed to submit proof to relayer at {endpoint}: {err}"))?;

        let status = response.status();
        let body = response.text().unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow!(
                "relayer submit failed with {} {}: {}",
                status.as_u16(),
                status,
                body
            ));
        }

        serde_json::from_str::<SubmitProofResponse>(&body)
            .map_err(|err| anyhow!("failed to decode relayer submit response: {err}; body={body}"))
    }

    fn wait_for_tx_hash(
        &self,
        relayer_api_url: &str,
        job_id: &str,
        timeout_secs: u64,
        poll_interval_ms: u64,
    ) -> Result<String> {
        let timeout = Duration::from_secs(timeout_secs);
        let poll_interval = Duration::from_millis(poll_interval_ms);
        let start = Instant::now();

        loop {
            let status = match self.fetch_job_status(relayer_api_url, job_id) {
                Ok(status) => status,
                Err(err) if super::is_retryable_request_error(&err) => {
                    if start.elapsed() >= timeout {
                        return Err(anyhow!(
                            "timed out waiting for tx hash on relayer job {} after {}s; last status query failed: {err:#}",
                            job_id,
                            timeout_secs
                        ));
                    }
                    std::thread::sleep(poll_interval);
                    continue;
                }
                Err(err) => return Err(err),
            };
            if let Some(tx_hash) = status.tx_hash {
                return Ok(tx_hash);
            }
            if status.status == JobStatus::Failed {
                return Err(anyhow!(
                    "relayer job {} failed before broadcast: {}",
                    status.job_id,
                    status
                        .last_error
                        .unwrap_or_else(|| "unknown error".to_string())
                ));
            }
            if start.elapsed() >= timeout {
                return Err(anyhow!(
                    "timed out waiting for tx hash on relayer job {} after {}s",
                    job_id,
                    timeout_secs
                ));
            }
            std::thread::sleep(poll_interval);
        }
    }

    fn wait_for_confirmation(
        &self,
        relayer_api_url: &str,
        job_id: &str,
        timeout_secs: u64,
        poll_interval_ms: u64,
    ) -> Result<RelayerConfirmation> {
        let timeout = Duration::from_secs(timeout_secs);
        let poll_interval = Duration::from_millis(poll_interval_ms);
        let start = Instant::now();

        loop {
            let status = match self.fetch_job_status(relayer_api_url, job_id) {
                Ok(status) => status,
                Err(err) if super::is_retryable_request_error(&err) => {
                    if start.elapsed() >= timeout {
                        return Err(anyhow!(
                            "timed out waiting for relayer job {} after {}s; last status query failed: {err:#}",
                            job_id,
                            timeout_secs
                        ));
                    }
                    std::thread::sleep(poll_interval);
                    continue;
                }
                Err(err) => return Err(err),
            };
            match status.status {
                JobStatus::Confirmed => {
                    return Ok(RelayerConfirmation {
                        job_id: status.job_id,
                        tx_hash: status.tx_hash,
                        block_number: status.block_number.map(|block_number| block_number as i64),
                    });
                }
                JobStatus::Failed => {
                    return Err(anyhow!(
                        "relayer job {} failed: {}",
                        status.job_id,
                        status
                            .last_error
                            .clone()
                            .unwrap_or_else(|| "unknown error".to_string())
                    ));
                }
                JobStatus::Queued | JobStatus::Sending | JobStatus::Submitted => {}
            }

            if start.elapsed() >= timeout {
                return Err(anyhow!(
                    "timed out waiting for relayer job {} after {}s",
                    job_id,
                    timeout_secs
                ));
            }
            std::thread::sleep(poll_interval);
        }
    }

    fn lookup_tx_hashes(
        &self,
        relayer_api_url: &str,
        tx_finals: &[Hash],
    ) -> Result<HashMap<Hash, String>> {
        let mut resolved = HashMap::new();
        if tx_finals.is_empty() {
            return Ok(resolved);
        }

        let endpoint = format!(
            "{}/api/v1/proofs/tx-hashes",
            relayer_api_url.trim_end_matches('/')
        );

        // Chunk so a large pending objects still resolves in one
        // `sync_objects` call, just spread across a few HTTP requests.
        for chunk in tx_finals.chunks(TX_HASH_BATCH_LIMIT) {
            let request = TxHashesByTxFinalRequest {
                tx_finals: chunk.to_vec(),
            };
            let response = self
                .client
                .post(&endpoint)
                .json(&request)
                .send()
                .map_err(|err| anyhow!("failed to query relayer tx hashes at {endpoint}: {err}"))?;

            let status = response.status();
            let body = response.text().unwrap_or_default();
            if !status.is_success() {
                return Err(anyhow!(
                    "relayer tx-hash lookup failed with {} {}: {}",
                    status.as_u16(),
                    status,
                    body
                ));
            }

            let payload: TxHashesByTxFinalResponse =
                serde_json::from_str(&body).map_err(|err| {
                    anyhow!("failed to decode relayer tx-hash response: {err}; body={body}")
                })?;
            for entry in payload.results {
                resolved.insert(entry.tx_final, entry.tx_hash);
            }
        }

        Ok(resolved)
    }
}
