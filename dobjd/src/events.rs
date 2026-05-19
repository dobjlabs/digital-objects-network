use serde::Serialize;
use tokio::sync::broadcast;
use wire_types::RunActionProgress;

/// A server-sent event broadcast over `/events`.
///
/// Frontend consumers switch on `type` to dispatch to the appropriate
/// handler. New variants are added as the corresponding routes are
/// ported.
///
/// `#[serde(tag = "type")]` produces internally-tagged JSON: a newtype
/// variant wrapping a struct flattens that struct's fields alongside the
/// discriminator, so `Event::RunActionProgress(progress)` serializes as
/// `{"type": "run-action-progress", "runId": ..., "phase": ..., ...}` —
/// matching the legacy Tauri payload plus a discriminator.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Event {
    /// Progress update for an in-flight `run_action` invocation.
    RunActionProgress(RunActionProgress),
}

/// Channel capacity for the broadcast hub. Slow subscribers get lagged events
/// dropped (they reconnect via EventSource auto-retry).
pub const CHANNEL_CAPACITY: usize = 256;

pub type EventTx = broadcast::Sender<Event>;
pub type EventRx = broadcast::Receiver<Event>;

pub fn channel() -> (EventTx, EventRx) {
    broadcast::channel(CHANNEL_CAPACITY)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wire_types::{ExecutionPhase, ProofProgressStatus};

    #[test]
    fn run_action_progress_serializes_flat_with_discriminator() {
        // Frontend + CLI consumers depend on the discriminator-plus-flat-
        // fields shape. Newtype variants in an internally-tagged enum
        // should produce exactly that — guard with an explicit test so a
        // future refactor doesn't silently break the wire format.
        let event = Event::RunActionProgress(RunActionProgress {
            run_id: "r1".to_string(),
            phase: ExecutionPhase::GenerateProof,
            status: ProofProgressStatus::Running,
            message: "msg".to_string(),
            old_root: None,
            new_root: None,
            output_files: None,
            output_status: None,
            nullified_files: None,
        });
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(
            json["type"], "run-action-progress",
            "discriminator key/value"
        );
        assert_eq!(json["runId"], "r1");
        assert_eq!(json["phase"], "generateProof");
        assert_eq!(json["status"], "running");
        assert!(
            json.get("oldRoot").is_some(),
            "Option<None> serializes to null"
        );
    }
}
