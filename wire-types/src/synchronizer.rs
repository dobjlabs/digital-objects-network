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
//! ([`ObjectProofResponse`] and [`GroundingWitnessResponse`]) embed a
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
    /// Number of objects in the canonical global created set.
    pub created_count: usize,
    /// Number of spent nullifiers in canonical state.
    pub nullifier_count: usize,
    /// Number of GSR entries in canonical history.
    pub gsr_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Batch created-object-membership request.
pub struct ObjectContainsRequest {
    /// Object commitments to look up in the canonical created set.
    pub object_commitments: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Membership result for one object commitment.
pub struct ObjectContainsEntry {
    /// Queried object commitment.
    pub commitment: String,
    /// Whether the object is present in the canonical created set.
    pub present: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Batch created-object-membership response.
pub struct ObjectContainsResponse {
    /// Last canonical slot fully committed by the synchronizer.
    pub last_processed_slot: u32,
    /// Current canonical global state root encoded as hex, if one exists.
    pub current_gsr: Option<String>,
    /// Per-commitment membership results.
    pub results: Vec<ObjectContainsEntry>,
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
/// Combined batch membership request for both created objects and nullifiers.
pub struct MembershipRequest {
    /// Object commitments to look up in the canonical created set.
    pub object_commitments: Vec<String>,
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
    /// Per-object created-set membership results.
    pub created_results: Vec<ObjectContainsEntry>,
    /// Per-nullifier membership results.
    pub nullifier_results: Vec<NullifierContainsEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Request for created-object grounding proofs used by txlib execution.
pub struct GroundingWitnessRequest {
    /// Input object commitments that must be proven present in the canonical created set.
    pub object_commitments: Vec<String>,
}

#[cfg(feature = "chain")]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Created-set membership proof response for one object.
///
/// `chain`-feature only — embeds a pod2 `MerkleProof`.
pub struct ObjectProofResponse {
    /// Object commitment the client asked about.
    pub commitment: String,
    /// Whether the object is present in the canonical created set.
    pub present: bool,
    /// Array index of the object in the created set. `None` when not present.
    pub index: Option<i64>,
    /// `ArrayContains` Merkle proof against the canonical created-set root.
    /// `None` when the object is not present (the array has no such leaf).
    pub proof: Option<MerkleProof>,
}

#[cfg(feature = "chain")]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// txlib witness response anchored to one canonical state root.
///
/// `chain`-feature only — embeds a `Vec<ObjectProofResponse>`.
pub struct GroundingWitnessResponse {
    /// Hash of the compact `txlib::StateRoot` built from the canonical roots.
    pub state_root_hash: String,
    /// Execution block number committed inside that state root.
    pub block_number: i64,
    /// Canonical created-set root encoded as hex.
    pub created_root: String,
    /// Canonical nullifiers set root encoded as hex.
    pub nullifiers_root: String,
    /// Prior-GSR array root committed inside the state root.
    pub gsrs_root: String,
    /// Per-input-object created-set membership proofs.
    pub created_proofs: Vec<ObjectProofResponse>,
}
