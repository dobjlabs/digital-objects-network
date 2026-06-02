use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use base64::{Engine, engine::general_purpose::STANDARD};
use common::blob::MAX_SIMPLE_BLOB_PAYLOAD_BYTES;
use wire_types::relayer::{JobStatus, JobStatusResponse, SubmitProofRequest, SubmitProofResponse};

pub const RELAYER_POLL_TIMEOUT_SECS: u64 = 180;
pub const RELAYER_POLL_INTERVAL_MS: u64 = 1500;

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
    /// Look up the current tx_hash for a job by its tx_final (proof commitment).
    /// Returns `Ok(None)` if the relayer has no record for this tx_final.
    fn lookup_tx_hash(&self, relayer_api_url: &str, tx_final: &str) -> Result<Option<String>>;
}

#[derive(Debug, Default)]
pub struct HttpRelayerClient;

impl HttpRelayerClient {
    fn fetch_job_status(&self, relayer_api_url: &str, job_id: &str) -> Result<JobStatusResponse> {
        let endpoint = format!(
            "{}/api/v1/proofs/{job_id}",
            relayer_api_url.trim_end_matches('/')
        );
        let client = reqwest::blocking::Client::new();
        let response = client
            .get(&endpoint)
            .send()
            .map_err(|err| anyhow!("failed to query relayer job at {endpoint}: {err}"))?;

        let status = response.status();
        let body = response.text().unwrap_or_default();
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

        let client = reqwest::blocking::Client::new();
        let response = client
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
            let status = self.fetch_job_status(relayer_api_url, job_id)?;
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
            let status = self.fetch_job_status(relayer_api_url, job_id)?;
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

    fn lookup_tx_hash(&self, relayer_api_url: &str, tx_final: &str) -> Result<Option<String>> {
        let endpoint = format!(
            "{}/api/v1/proofs/by-tx-final/{tx_final}",
            relayer_api_url.trim_end_matches('/')
        );
        let client = reqwest::blocking::Client::new();
        let response = client
            .get(&endpoint)
            .send()
            .map_err(|err| anyhow!("failed to query relayer by tx_final at {endpoint}: {err}"))?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        let status = response.status();
        let body = response.text().unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow!(
                "relayer lookup by tx_final failed with {} {}: {}",
                status.as_u16(),
                status,
                body
            ));
        }

        let job: JobStatusResponse = serde_json::from_str(&body)
            .map_err(|err| anyhow!("failed to decode relayer response: {err}; body={body}"))?;
        Ok(job.tx_hash)
    }
}
