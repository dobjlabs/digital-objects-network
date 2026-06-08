use pod2::middleware::Hash;
use serde::{Deserialize, Serialize};

pub use wire_types::relayer::JobStatus;

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
    pub tx_final: Hash,
    /// State root hash claimed by the payload.
    pub state_root_hash: Hash,
    /// Optional caller-provided trace string for observability.
    pub client_ref: Option<String>,
    /// Total submission attempts made so far.
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
    /// Ethereum nonce used for current TX (needed for RBF replacement).
    pub nonce: Option<i64>,
    /// Number of fee-bump replacements applied for this job.
    pub bump_count: i32,
    /// Previous tx hashes replaced by fee bumps (most recent last).
    pub prev_tx_hashes: Vec<String>,
    /// Creation timestamp (unix seconds).
    pub created_at: i64,
    /// Last update timestamp (unix seconds).
    pub updated_at: i64,
}
