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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DashboardStatus {
    Healthy,
    Lagging,
    Recovering,
    Stalled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DashboardSlotStatus {
    Pending,
    Applied,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardSummaryResponse {
    pub status: DashboardStatus,
    pub status_reason: String,
    pub last_processed_slot: Option<u32>,
    pub beacon_head_slot: Option<u32>,
    pub slot_lag: Option<u32>,
    pub beacon_head_block_number: Option<u32>,
    pub block_lag: Option<u32>,
    pub last_processed_block_number: Option<u32>,
    pub current_block_number: Option<i64>,
    pub current_gsr: Option<String>,
    pub tx_count: usize,
    pub nullifier_count: usize,
    pub gsr_count: usize,
    pub pending_recovery_count: usize,
    pub cursor_updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardRecentSlotRow {
    pub slot: u32,
    pub execution_block_number: Option<u32>,
    pub status: DashboardSlotStatus,
    pub is_empty: bool,
    pub block_root: Option<String>,
    pub parent_root: Option<String>,
    pub tx_count: usize,
    pub nullifier_count: usize,
    pub gsr_hash: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardRecentSlotsResponse {
    pub slots: Vec<DashboardRecentSlotRow>,
}
