//! Transaction records stored beside payloads in `transactions/<txFinal>/`.
//! Hex hashes allow the same record format on disk and over the wire without
//! enabling chain dependencies.

use std::fmt;

use serde::{Deserialize, Serialize};

#[cfg(feature = "schemars")]
use schemars::JsonSchema;

use crate::QualifiedName;

/// Transaction lifecycle; `Landed` and `Expired` are terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum TransactionState {
    /// Stored locally; relayer acceptance has not been recorded.
    Recorded,
    /// Relayer acceptance recorded with a job ID.
    Submitted,
    /// Every created commitment and nullifier is in the synchronizer's sets.
    Landed,
    /// Absent from the chain after the grounding age limit; can no longer land.
    Expired,
}

impl TransactionState {
    pub fn is_terminal(self) -> bool {
        matches!(self, TransactionState::Landed | TransactionState::Expired)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            TransactionState::Recorded => "recorded",
            TransactionState::Submitted => "submitted",
            TransactionState::Landed => "landed",
            TransactionState::Expired => "expired",
        }
    }
}

impl fmt::Display for TransactionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct TransactionRecord {
    /// Finalized transaction commitment, used as the record's key.
    pub tx_final: String,
    pub action: QualifiedName,
    /// Proof owner label, reported as the input holder while the record is open.
    pub owner: String,
    /// Basenames of the object files this transaction spends.
    pub input_files: Vec<String>,
    /// Basenames of the object files this transaction creates.
    pub output_files: Vec<String>,
    /// State root the transaction is grounded against.
    pub grounding_root: String,
    /// Execution block number of the grounding root.
    pub grounding_block: i64,
    /// Payload nullifiers.
    pub nullifiers: Vec<String>,
    /// Payload commitments, including intermediate states.
    pub created: Vec<String>,
    pub state: TransactionState,
    /// Relayer job ID, set on acceptance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relayer_job_id: Option<String>,
    /// Ethereum transaction hash, once the relayer has broadcast the blob.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tx_hash: Option<String>,
    /// Relayer failure reason. A failed job may still land; this does not
    /// determine the transaction's state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relayer_failure: Option<String>,
    /// Explanation of the terminal state, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub recorded_at_ms: u64,
    /// First submission attempt time. Once set, deletion is refused even
    /// without a job ID: the relayer may have accepted the payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submit_attempted_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submitted_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settled_at_ms: Option<u64>,
    /// Outputs written under a temporary name because they take over a
    /// mutated input's custom name once the transaction lands.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_names: Vec<PendingName>,
}

/// An output file written as `file_name` that takes the name `final_name`,
/// held until then by the input it replaces, when its transaction lands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct PendingName {
    pub file_name: String,
    pub final_name: String,
}

impl TransactionRecord {
    /// Initialize a recorded transaction with no submission or settlement data.
    #[allow(clippy::too_many_arguments)]
    pub fn recorded(
        tx_final: String,
        action: QualifiedName,
        owner: String,
        input_files: Vec<String>,
        output_files: Vec<String>,
        grounding_root: String,
        grounding_block: i64,
        nullifiers: Vec<String>,
        created: Vec<String>,
        recorded_at_ms: u64,
    ) -> Self {
        Self {
            tx_final,
            action,
            owner,
            input_files,
            output_files,
            grounding_root,
            grounding_block,
            nullifiers,
            created,
            state: TransactionState::Recorded,
            relayer_job_id: None,
            tx_hash: None,
            relayer_failure: None,
            reason: None,
            recorded_at_ms,
            submit_attempted_at_ms: None,
            submitted_at_ms: None,
            settled_at_ms: None,
            pending_names: Vec::new(),
        }
    }

    pub fn is_open(&self) -> bool {
        !self.state.is_terminal()
    }

    /// Whether settlement must ask the synchronizer for this transaction's status: the
    /// relayer may hold its payload once a submission was attempted.
    pub fn needs_tx_status(&self) -> bool {
        self.is_open()
            && (self.submit_attempted_at_ms.is_some() || self.state == TransactionState::Submitted)
    }
}
