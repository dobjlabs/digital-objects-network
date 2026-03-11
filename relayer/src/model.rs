use anyhow::anyhow;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Sending,
    Submitted,
    Confirmed,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayJob {
    pub job_id: String,
    pub status: JobStatus,
    pub payload_bytes: Vec<u8>,
    pub tx_final: String,
    pub state_root_hash: String,
    pub client_ref: Option<String>,
    pub attempt_count: u32,
    pub tx_hash: Option<String>,
    pub submitted_at: Option<i64>,
    pub block_number: Option<u64>,
    pub last_error: Option<String>,
    pub next_attempt_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}
