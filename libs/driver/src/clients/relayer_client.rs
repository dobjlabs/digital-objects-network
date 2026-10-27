use anyhow::{Result, anyhow};
use base64::{Engine, engine::general_purpose::STANDARD};
use payload::blob::MAX_SIMPLE_BLOB_PAYLOAD_BYTES;
use pod2::middleware::Hash;
use wire_types::relayer::{
    JobStatusResponse, SubmitProofRequest, SubmitProofResponse, TxHashesByTxFinalRequest,
    TxHashesByTxFinalResponse,
};

pub trait RelayerClient: Send + Sync {
    fn submit_proof(
        &self,
        relayer_api_url: &str,
        payload_bytes: &[u8],
        client_ref: Option<String>,
    ) -> Result<SubmitProofResponse>;
    fn fetch_job_status(&self, relayer_api_url: &str, job_id: &str) -> Result<JobStatusResponse>;
    fn fetch_tx_hashes(
        &self,
        relayer_api_url: &str,
        tx_finals: &[Hash],
    ) -> Result<TxHashesByTxFinalResponse>;
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
    fn fetch_tx_hashes(
        &self,
        relayer_api_url: &str,
        tx_finals: &[Hash],
    ) -> Result<TxHashesByTxFinalResponse> {
        let endpoint = format!(
            "{}/api/v1/proofs/tx-hashes",
            relayer_api_url.trim_end_matches('/')
        );
        super::send_json_request(
            self.client.post(&endpoint).json(&TxHashesByTxFinalRequest {
                tx_finals: tx_finals.to_vec(),
            }),
            &endpoint,
            "relayer transaction hashes response",
        )
    }

    fn fetch_job_status(&self, relayer_api_url: &str, job_id: &str) -> Result<JobStatusResponse> {
        let endpoint = format!(
            "{}/api/v1/proofs/{job_id}",
            relayer_api_url.trim_end_matches('/')
        );
        super::send_json_request(
            self.client.get(&endpoint),
            &endpoint,
            "relayer status response",
        )
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

        super::send_json_request(
            self.client.post(&endpoint).json(&request),
            &endpoint,
            "relayer submit response",
        )
    }
}
