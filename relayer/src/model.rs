use anyhow::anyhow;
use serde::{Deserialize, Serialize};

/// Persistent relay lifecycle states.
///
/// Typical happy path:
/// `queued -> sending -> submitted -> confirmed`.
/// Failures/retries can bounce back to `queued` or end in `failed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
    /// Canonical lowercase value stored in Postgres and exposed in API JSON.
    pub fn as_str(self) -> &'static str {
        match self {
            JobStatus::Queued => "queued",
            JobStatus::Sending => "sending",
            JobStatus::Submitted => "submitted",
            JobStatus::Confirmed => "confirmed",
            JobStatus::Failed => "failed",
        }
    }

    /// `true` when no further worker processing should occur.
    pub fn is_terminal(self) -> bool {
        matches!(self, JobStatus::Confirmed | JobStatus::Failed)
    }

    /// Parse a status loaded from Postgres.
    pub fn from_db_str(value: &str) -> anyhow::Result<Self> {
        match value {
            "queued" => Ok(JobStatus::Queued),
            "sending" => Ok(JobStatus::Sending),
            "submitted" => Ok(JobStatus::Submitted),
            "confirmed" => Ok(JobStatus::Confirmed),
            "failed" => Ok(JobStatus::Failed),
            _ => Err(anyhow!("invalid job status: {value}")),
        }
    }
}

/// Durable relay job record stored in `relay_jobs`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayJob {
    /// Stable API identifier (UUID string).
    pub job_id: String,
    /// Current lifecycle status.
    pub status: JobStatus,
    /// Raw verified payload bytes to submit as EIP-4844 blob data.
    pub payload_bytes: Vec<u8>,
    /// Idempotency key derived from proof payload.
    pub tx_final: String,
    /// State root hash claimed by the payload.
    pub state_root_hash: String,
    /// Optional caller-provided trace string for observability.
    pub client_ref: Option<String>,
    /// Total worker attempts made so far (submit + poll attempts).
    pub attempt_count: u32,
    /// Ethereum tx hash once broadcast succeeds.
    pub tx_hash: Option<String>,
    /// First successful submit timestamp (unix seconds).
    pub submitted_at: Option<i64>,
    /// Receipt block number when known.
    pub block_number: Option<u64>,
    /// Last failure reason shown to API clients.
    pub last_error: Option<String>,
    /// Next timestamp when worker may process this job.
    pub next_attempt_at: Option<i64>,
    /// Creation timestamp (unix seconds).
    pub created_at: i64,
    /// Last update timestamp (unix seconds).
    pub updated_at: i64,
}
