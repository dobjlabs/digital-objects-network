//! Synchronizer HTTP API DTOs.
//!
//! Lifted out of the `synchronizer` crate so consumers (driver, dobjd,
//! tests) can deserialize synchronizer responses without pulling in
//! synchronizer's server-side deps (rocksdb, sqlx-postgres, alloy, axum).
//! That's the entire point -- `rocksdb` requires a C++ toolchain and is
//! the dominant Windows-build headache; keeping it on the server side
//! lets `dobjd` build clean on Windows.
//!
//! Every DTO here carries pod2 `Hash` values directly (serialized as the
//! 64-char hex pod2 emits), so the whole module lives behind the `chain`
//! feature. Only the synchronizer server and the driver speak this API,
//! and both enable the feature.

use pod2::backends::plonky2::primitives::merkletree::MerkleProof;
use pod2::middleware::Hash;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Synchronization progress returned by the synchronizer API.
pub struct SyncProgressResponse {
    /// Last slot fully committed by the synchronizer.
    pub last_processed_slot: u32,
    /// Execution block number associated with the last processed slot, if any.
    pub last_processed_block_number: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Summary of the current state head exposed to clients.
pub struct StateHeadResponse {
    /// Last slot fully committed by the synchronizer.
    pub last_processed_slot: u32,
    /// Execution block number associated with the last processed slot, if any.
    pub last_processed_block_number: Option<u32>,
    /// Current state root, if one exists.
    pub current_state_root: Option<Hash>,
    /// Execution block number committed inside the current state root, if any.
    pub current_block_number: Option<i64>,
    /// Number of objects in the global created set.
    pub created_count: usize,
    /// Number of spent nullifiers in committed state.
    pub nullifier_count: usize,
    /// Number of state root entries in the state history.
    pub state_root_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Batch created-object-membership request.
pub struct ObjectContainsRequest {
    /// Object commitments to look up in the created set.
    pub object_commitments: Vec<Hash>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Membership result for one object commitment.
pub struct ObjectContainsEntry {
    /// Queried object commitment.
    pub commitment: Hash,
    /// Whether the object is present in the created set.
    pub present: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Batch created-object-membership response.
pub struct ObjectContainsResponse {
    /// Last slot fully committed by the synchronizer.
    pub last_processed_slot: u32,
    /// Current state root, if one exists.
    pub current_state_root: Option<Hash>,
    /// Per-commitment membership results.
    pub results: Vec<ObjectContainsEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Batch nullifier-membership request.
pub struct NullifierContainsRequest {
    /// Nullifier hashes to look up in the nullifiers set.
    pub nullifiers: Vec<Hash>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Membership result for one nullifier hash.
pub struct NullifierContainsEntry {
    /// Queried nullifier hash.
    pub nullifier: Hash,
    /// Whether the hash is present in the nullifiers set.
    pub present: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Batch nullifier-membership response.
pub struct NullifierContainsResponse {
    /// Last slot fully committed by the synchronizer.
    pub last_processed_slot: u32,
    /// Current state root, if one exists.
    pub current_state_root: Option<Hash>,
    /// Per-hash membership results.
    pub results: Vec<NullifierContainsEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Combined batch membership request for both created objects and nullifiers.
pub struct MembershipRequest {
    /// Object commitments to look up in the created set.
    pub object_commitments: Vec<Hash>,
    /// Nullifier hashes to look up in the nullifiers set.
    pub nullifiers: Vec<Hash>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Combined batch membership response anchored to one state head.
pub struct MembershipResponse {
    /// Last slot fully committed by the synchronizer.
    pub last_processed_slot: u32,
    /// Current state root, if one exists.
    pub current_state_root: Option<Hash>,
    /// Per-object created-set membership results.
    pub created_results: Vec<ObjectContainsEntry>,
    /// Per-nullifier membership results.
    pub nullifier_results: Vec<NullifierContainsEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Request for created-object grounding proofs used by txlib execution.
pub struct GroundingWitnessRequest {
    /// Input object commitments that must be proven present in the created set.
    pub object_commitments: Vec<Hash>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Created-set membership proof response for one object.
pub struct ObjectProofResponse {
    /// Object commitment the client asked about.
    pub commitment: Hash,
    /// Whether the object is present in the created set.
    pub present: bool,
    /// Array index of the object in the created set. `None` when not present.
    pub index: Option<i64>,
    /// `ArrayContains` Merkle proof against the created-set root.
    /// `None` when the object is not present (the array has no such leaf).
    pub proof: Option<MerkleProof>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// txlib witness response anchored to one state root.
pub struct GroundingWitnessResponse {
    /// Hash of the compact `txlib::StateHeader` built from the state roots.
    pub state_root: Hash,
    /// Execution block number committed inside that state root.
    pub block_number: i64,
    /// Created-set root.
    pub created_root: Hash,
    /// Nullifiers set root.
    pub nullifiers_root: Hash,
    /// Prior-state root array root committed inside the state root.
    pub prior_state_history_root: Hash,
    /// Per-input-object created-set membership proofs.
    pub created_proofs: Vec<ObjectProofResponse>,
}
