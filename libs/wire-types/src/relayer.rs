//! Relayer HTTP API DTOs.
//!
//! Lifted out of the `relayer` crate so consumers (driver, dobjd, tests)
//! can deserialize relayer responses without pulling in relayer's
//! server-side deps (sqlx-postgres, alloy, axum).
//!
//! The status/request types are pure serde. The two response types carry
//! pod2 `Hash` (`tx_final`, `state_root`, serialized as the 64-char
//! hex pod2 emits), so they live behind the `chain` feature; `tx_hash`
//! stays a `String` because it is an Ethereum keccak hash, not a pod2 one.

use serde::{Deserialize, Serialize};

#[cfg(feature = "chain")]
use pod2::middleware::Hash;

/// Persistent relay lifecycle states.
///
/// Typical happy path:
/// `queued -> sending -> submitted -> confirmed`.
/// Failures/retries can bounce back to `queued` or end in `failed`.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum JobStatus {
    /// Accepted by API and waiting for the next worker attempt window.
    Queued,
    /// Worker is currently attempting to broadcast the blob transaction.
    Sending,
    /// Broadcast succeeded; worker is polling receipts by tx hash.
    Submitted,
    /// Receipt confirmed successful execution.
    Confirmed,
    /// Terminal failure (max retries, timeout, permanent error, or revert).
    Failed,
}

impl JobStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            JobStatus::Queued => "queued",
            JobStatus::Sending => "sending",
            JobStatus::Submitted => "submitted",
            JobStatus::Confirmed => "confirmed",
            JobStatus::Failed => "failed",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, JobStatus::Confirmed | JobStatus::Failed)
    }

    /// Parse a relayer-DB string representation. Lives here (not in the
    /// relayer crate) so the driver can also map DB-style strings if it
    /// ever needs to — e.g. when ingesting cached relayer state.
    pub fn from_db_str(value: &str) -> Result<Self, String> {
        match value {
            "queued" => Ok(JobStatus::Queued),
            "sending" => Ok(JobStatus::Sending),
            "submitted" => Ok(JobStatus::Submitted),
            "confirmed" => Ok(JobStatus::Confirmed),
            "failed" => Ok(JobStatus::Failed),
            other => Err(format!("invalid job status: {other}")),
        }
    }
}

/// Client submit payload for creating/looking up a relay job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitProofRequest {
    /// Base64-encoded binary proof payload expected by `payload::proof::BlobParser`.
    pub payload_base64: String,
    /// Optional caller-supplied reference stored with the job for tracing.
    pub client_ref: Option<String>,
}

/// Batch request to resolve current Ethereum tx hashes for a set of proof
/// commitments (`tx_final`). Used to refresh hashes that may have changed via
/// fee-bump replacement.
#[cfg(feature = "chain")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxHashesByTxFinalRequest {
    /// Proof commitments to resolve.
    pub tx_finals: Vec<Hash>,
}

/// One resolved `(tx_final, current Ethereum tx hash)` pair.
#[cfg(feature = "chain")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxHashEntry {
    /// Proof commitment the job is keyed by.
    pub tx_final: Hash,
    /// Current Ethereum tx hash the relayer has broadcast for it.
    pub tx_hash: String,
}

/// Batch response: one entry per requested `tx_final` that has a known,
/// broadcast tx hash. Unknown or not-yet-broadcast commitments are omitted.
#[cfg(feature = "chain")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxHashesByTxFinalResponse {
    /// Resolved hashes, in no particular order.
    pub results: Vec<TxHashEntry>,
}

/// Submit response returns the created/existing job identity and key metadata.
#[cfg(feature = "chain")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitProofResponse {
    /// Stable job id used for status polling.
    pub job_id: String,
    /// Current lifecycle status of the returned job.
    pub status: JobStatus,
    /// Idempotency key derived from the decoded payload.
    pub tx_final: Hash,
    /// State root hash claimed by the payload.
    pub state_root: Hash,
    /// Submission attempts observed so far for this job.
    pub attempt_count: u32,
    /// Job creation timestamp in unix seconds.
    pub created_at: i64,
}

/// Status response is the durable view of worker progress for one job.
#[cfg(feature = "chain")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobStatusResponse {
    /// Stable job id used for status polling.
    pub job_id: String,
    /// Current lifecycle status.
    pub status: JobStatus,
    /// Ethereum tx hash once broadcast has succeeded.
    pub tx_hash: Option<String>,
    /// Receipt block number when known.
    pub block_number: Option<u64>,
    /// Total submission attempts made so far by the worker.
    pub attempt_count: u32,
    /// Most recent failure reason, if any.
    pub last_error: Option<String>,
    /// Last update timestamp in unix seconds.
    pub updated_at: i64,
    /// Creation timestamp in unix seconds.
    pub created_at: i64,
    /// Idempotency key derived from the decoded payload.
    pub tx_final: Hash,
    /// State root hash claimed by the payload.
    pub state_root: Hash,
}
