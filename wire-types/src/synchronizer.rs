//! Synchronizer HTTP API DTOs.
//!
//! Lifted out of the `synchronizer` crate so consumers (driver, dobjd,
//! tests) can deserialize synchronizer responses without pulling in
//! synchronizer's server-side deps (rocksdb, sqlx-postgres, alloy, axum).
//! That's the entire point — `rocksdb` requires a C++ toolchain and is
//! the dominant Windows-build headache; keeping it on the server side
//! lets `dobjd` build clean on Windows.
//!
//! Most types here are pure serde DTOs. The two proof-bearing types
//! ([`SourceTxProofResponse`] and [`GroundingWitnessResponse`]) embed a
//! pod2 `MerkleProof`, so they live behind the `chain` feature. Server
//! code and the driver enable the feature; `cli` and `mcp` don't.

use serde::{Deserialize, Serialize};

#[cfg(feature = "chain")]
use pod2::backends::plonky2::primitives::merkletree::MerkleProof;

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Liveness response for the synchronizer HTTP server.
pub struct HealthResponse {
    /// Whether the server is up and responding.
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Synchronization progress returned by the synchronizer API.
pub struct SyncProgressResponse {
    /// Last canonical slot fully committed by the synchronizer.
    pub last_processed_slot: u32,
    /// Execution block number associated with the last processed slot, if any.
    pub last_processed_block_number: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Summary of the current canonical head exposed to clients.
pub struct StateHeadResponse {
    /// Last canonical slot fully committed by the synchronizer.
    pub last_processed_slot: u32,
    /// Execution block number associated with the last processed slot, if any.
    pub last_processed_block_number: Option<u32>,
    /// Current canonical global state root encoded as hex, if one exists.
    pub current_gsr: Option<String>,
    /// Execution block number committed inside the current state root, if any.
    pub current_block_number: Option<i64>,
    /// Number of accepted transactions in canonical state.
    pub tx_count: usize,
    /// Number of spent nullifiers in canonical state.
    pub nullifier_count: usize,
    /// Number of GSR entries in canonical history.
    pub gsr_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Batch transaction-membership request.
pub struct TxContainsRequest {
    /// Transaction hashes to look up in the canonical transactions set.
    pub tx_hashes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Membership result for one transaction hash.
pub struct TxContainsEntry {
    /// Queried transaction hash.
    pub tx_hash: String,
    /// Whether the hash is present in the canonical transactions set.
    pub present: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Batch transaction-membership response.
pub struct TxContainsResponse {
    /// Last canonical slot fully committed by the synchronizer.
    pub last_processed_slot: u32,
    /// Current canonical global state root encoded as hex, if one exists.
    pub current_gsr: Option<String>,
    /// Per-hash membership results.
    pub results: Vec<TxContainsEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Batch nullifier-membership request.
pub struct NullifierContainsRequest {
    /// Nullifier hashes to look up in the canonical nullifiers set.
    pub nullifiers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Membership result for one nullifier hash.
pub struct NullifierContainsEntry {
    /// Queried nullifier hash.
    pub nullifier: String,
    /// Whether the hash is present in the canonical nullifiers set.
    pub present: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Batch nullifier-membership response.
pub struct NullifierContainsResponse {
    /// Last canonical slot fully committed by the synchronizer.
    pub last_processed_slot: u32,
    /// Current canonical global state root encoded as hex, if one exists.
    pub current_gsr: Option<String>,
    /// Per-hash membership results.
    pub results: Vec<NullifierContainsEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Combined batch membership request for both transactions and nullifiers.
pub struct MembershipRequest {
    /// Transaction hashes to look up in the canonical transactions set.
    pub tx_hashes: Vec<String>,
    /// Nullifier hashes to look up in the canonical nullifiers set.
    pub nullifiers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Combined batch membership response anchored to one canonical head.
pub struct MembershipResponse {
    /// Last canonical slot fully committed by the synchronizer.
    pub last_processed_slot: u32,
    /// Current canonical global state root encoded as hex, if one exists.
    pub current_gsr: Option<String>,
    /// Per-transaction membership results.
    pub tx_results: Vec<TxContainsEntry>,
    /// Per-nullifier membership results.
    pub nullifier_results: Vec<NullifierContainsEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Point lookup response for one transaction hash.
pub struct TxStatusResponse {
    /// Queried transaction hash.
    pub tx_hash: String,
    /// Whether the hash is present in the canonical transactions set.
    pub present: bool,
    /// Last canonical slot fully committed by the synchronizer.
    pub last_processed_slot: u32,
    /// Current canonical global state root encoded as hex, if one exists.
    pub current_gsr: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Request for transaction-grounding proofs used by txlib execution.
pub struct GroundingWitnessRequest {
    /// Source transaction hashes that must be proven against the canonical transactions set.
    pub source_tx_hashes: Vec<String>,
}

#[cfg(feature = "chain")]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Membership proof response for one source transaction.
///
/// `chain`-feature only — embeds a pod2 `MerkleProof`.
pub struct SourceTxProofResponse {
    /// Source transaction hash the client asked about.
    pub tx_hash: String,
    /// Whether the source transaction is present in the canonical transactions set.
    pub present: bool,
    /// Merkle proof against the canonical transactions root.
    pub proof: MerkleProof,
}

#[cfg(feature = "chain")]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// txlib witness response anchored to one canonical state root.
///
/// `chain`-feature only — embeds a `Vec<SourceTxProofResponse>`.
pub struct GroundingWitnessResponse {
    /// Hash of the compact `txlib::StateRoot` built from the canonical roots.
    pub state_root_hash: String,
    /// Execution block number committed inside that state root.
    pub block_number: i64,
    /// Canonical transactions set root encoded as hex.
    pub transactions_root: String,
    /// Canonical nullifiers set root encoded as hex.
    pub nullifiers_root: String,
    /// Prior-GSR array root committed inside the state root.
    pub gsrs_root: String,
    /// Per-source transaction membership proofs.
    pub source_tx_proofs: Vec<SourceTxProofResponse>,
}
