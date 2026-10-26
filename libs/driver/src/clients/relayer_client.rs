use anyhow::{Context, Result, anyhow};
use base64::{Engine, engine::general_purpose::STANDARD};
use payload::blob::MAX_SIMPLE_BLOB_PAYLOAD_BYTES;
use wire_types::relayer::{JobStatusResponse, SubmitProofRequest, SubmitProofResponse};

pub trait RelayerClient: Send + Sync {
    fn submit_proof(
        &self,
        relayer_api_url: &str,
        payload_bytes: &[u8],
        client_ref: Option<String>,
    ) -> Result<SubmitProofResponse>;
    fn fetch_job_status(&self, relayer_api_url: &str, job_id: &str) -> Result<JobStatusResponse>;
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
}

impl RelayerClient for HttpRelayerClient {
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
}
