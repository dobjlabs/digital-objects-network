use pod2::backends::plonky2::primitives::merkletree::MerkleProof;
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
pub struct NullifierContainsRequest {
    pub nullifiers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NullifierContainsEntry {
    pub nullifier: String,
    pub present: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NullifierContainsResponse {
    pub last_processed_slot: Option<u32>,
    pub current_gsr: Option<String>,
    pub results: Vec<NullifierContainsEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MembershipRequest {
    pub tx_hashes: Vec<String>,
    pub nullifiers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MembershipResponse {
    pub last_processed_slot: Option<u32>,
    pub current_gsr: Option<String>,
    pub tx_results: Vec<TxContainsEntry>,
    pub nullifier_results: Vec<NullifierContainsEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxStatusResponse {
    pub tx_hash: String,
    pub present: bool,
    pub last_processed_slot: Option<u32>,
    pub current_gsr: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroundingWitnessRequest {
    pub source_tx_hashes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceTxProofResponse {
    pub tx_hash: String,
    pub present: bool,
    pub proof: MerkleProof,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroundingWitnessResponse {
    pub state_root_hash: String,
    pub block_number: i64,
    pub transactions_root: String,
    pub nullifiers_root: String,
    pub gsrs_root: String,
    pub source_tx_proofs: Vec<SourceTxProofResponse>,
}
