use std::time::{Duration, Instant};

use base64::{engine::general_purpose::STANDARD, Engine};
use common::blob::MAX_SIMPLE_BLOB_PAYLOAD_BYTES;
use serde::{Deserialize, Serialize};

pub(super) const RELAYER_POLL_TIMEOUT_SECS: u64 = 180;
pub(super) const RELAYER_POLL_INTERVAL_MS: u64 = 1500;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum RelayerJobStatus {
    Queued,
    Sending,
    Submitted,
    Confirmed,
    Failed,
}

impl RelayerJobStatus {
    pub(super) fn as_str(self) -> &'static str {
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
pub(super) struct RelayerSubmitResponse {
    pub(super) job_id: String,
    pub(super) status: RelayerJobStatus,
}

#[derive(Debug, Deserialize)]
pub(super) struct RelayerJobStatusResponse {
    pub(super) job_id: String,
    pub(super) status: RelayerJobStatus,
    pub(super) tx_hash: Option<String>,
    pub(super) last_error: Option<String>,
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

pub(super) fn submit_proof_to_relayer(
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

pub(super) fn wait_for_relayer_confirmation(
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
