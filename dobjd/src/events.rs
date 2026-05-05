use serde::Serialize;
use tokio::sync::broadcast;

use crate::progress::{ProofPhase, ProofProgressStatus};

/// A server-sent event broadcast over `/events`.
///
/// Frontend consumers switch on `type` to dispatch to the appropriate handler.
/// New variants are added as the corresponding routes are ported.
///
/// `serde(tag = "type")` produces internally-tagged JSON, so struct variants
/// emit `{"type": "...", ...inline fields...}` — matching the legacy Tauri
/// payload plus a discriminator.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Event {
    /// The objects directory changed on disk (file added / removed / modified).
    ObjectsChanged,
    /// Progress update for an in-flight `run_action` invocation.
    /// Field shape mirrors `RunActionProgress` from the legacy Tauri client.
    RunActionProgress {
        #[serde(rename = "runId")]
        run_id: String,
        phase: ProofPhase,
        status: ProofProgressStatus,
        message: String,
        #[serde(rename = "oldRoot")]
        old_root: Option<String>,
        #[serde(rename = "newRoot")]
        new_root: Option<String>,
        #[serde(rename = "outputFiles")]
        output_files: Option<Vec<String>>,
    },
}

/// Channel capacity for the broadcast hub. Slow subscribers get lagged events
/// dropped (they reconnect via EventSource auto-retry).
pub const CHANNEL_CAPACITY: usize = 256;

pub type EventTx = broadcast::Sender<Event>;
pub type EventRx = broadcast::Receiver<Event>;

pub fn channel() -> (EventTx, EventRx) {
    broadcast::channel(CHANNEL_CAPACITY)
}
