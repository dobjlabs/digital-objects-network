use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncProgressResponse {
    pub last_processed_slot: Option<u32>,
    pub last_processed_block_number: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateHeadResponse {
    pub last_processed_slot: Option<u32>,
    pub last_processed_block_number: Option<u32>,
    pub current_gsr: Option<String>,
    pub current_block_number: Option<i64>,
    pub tx_count: usize,
    pub nullifier_count: usize,
    pub gsr_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateFullResponse {
    pub block_number: i64,
    pub current_gsr: Option<String>,
    pub transactions: Vec<String>,
    pub nullifiers: Vec<String>,
    pub gsrs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxContainsRequest {
    pub tx_hashes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxContainsEntry {
    pub tx_hash: String,
    pub present: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxContainsResponse {
    pub last_processed_slot: Option<u32>,
    pub current_gsr: Option<String>,
    pub results: Vec<TxContainsEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxStatusResponse {
    pub tx_hash: String,
    pub present: bool,
    pub last_processed_slot: Option<u32>,
    pub current_gsr: Option<String>,
}
