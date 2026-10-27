//! Sequenced state-change events for `GET /events`.

use std::sync::{Arc, Mutex};

use serde::Serialize;
use tokio::sync::broadcast;
use wire_types::transactions::TransactionRecord;
use wire_types::{ObjectSummary, ProofJobState, RunActionProgress};

/// Events use a flat JSON object with a `type` discriminator and payload fields.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Event {
    RunActionProgress(RunActionProgress),
    ProofJob(ProofJobState),
    Transaction(TransactionRecord),
    #[serde(rename_all = "camelCase")]
    TransactionDeleted {
        tx_final: String,
    },
    #[serde(rename_all = "camelCase")]
    TransactionPruned {
        tx_final: String,
    },
    Object(ObjectSummary),
    /// A file removed by expiry, transaction deletion, or discard.
    #[serde(rename_all = "camelCase")]
    ObjectRemoved {
        file_name: String,
    },
}

/// Event payload with its stream sequence number, serialized as one flat object.
#[derive(Debug, Clone, Serialize)]
pub struct Envelope {
    pub seq: u64,
    #[serde(flatten)]
    pub event: Event,
}

/// Maximum buffered events per subscriber. Lagging subscribers lose older events.
pub const CHANNEL_CAPACITY: usize = 1024;

#[derive(Clone)]
pub struct EventHub {
    /// Held across the send so subscribers receive sequence numbers in order.
    next_seq: Arc<Mutex<u64>>,
    tx: broadcast::Sender<Envelope>,
}

impl Default for EventHub {
    fn default() -> Self {
        Self::new()
    }
}

impl EventHub {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(CHANNEL_CAPACITY);
        Self {
            next_seq: Arc::new(Mutex::new(1)),
            tx,
        }
    }

    /// Publish an event even if no clients are subscribed.
    pub fn publish(&self, event: Event) {
        let mut next_seq = self.next_seq.lock().expect("event sequence lock poisoned");
        let _ = self.tx.send(Envelope {
            seq: *next_seq,
            event,
        });
        *next_seq += 1;
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Envelope> {
        self.tx.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wire_types::{ExecutionPhase, ProofProgressStatus};

    #[test]
    fn events_serialize_flat_with_discriminator_and_sequence() {
        let hub = EventHub::new();
        let mut rx = hub.subscribe();
        hub.publish(Event::RunActionProgress(RunActionProgress {
            run_id: "r1".to_string(),
            phase: ExecutionPhase::GenerateProof,
            status: ProofProgressStatus::Running,
            message: "msg".to_string(),
            old_root: None,
            output_files: None,
            output_status: None,
            input_files: None,
            at_ms: 0,
            stage: None,
            elapsed_ms: 0,
        }));
        hub.publish(Event::ObjectRemoved {
            file_name: "gone.dobj".to_string(),
        });
        let first = serde_json::to_value(rx.try_recv().unwrap()).unwrap();
        assert_eq!(first["seq"], 1);
        assert_eq!(first["type"], "run-action-progress");
        assert_eq!(first["runId"], "r1");
        assert_eq!(first["phase"], "generateProof");
        assert_eq!(first["status"], "running");
        assert!(
            first.get("oldRoot").is_some(),
            "Option<None> serializes to null"
        );
        let second = serde_json::to_value(rx.try_recv().unwrap()).unwrap();
        assert_eq!(second["seq"], 2);
        assert_eq!(second["type"], "object-removed");
        assert_eq!(second["fileName"], "gone.dobj");
    }

    #[test]
    fn concurrent_publishers_deliver_consecutive_sequence_numbers() {
        let hub = EventHub::new();
        let mut rx = hub.subscribe();
        let publishers = (0..4)
            .map(|_| {
                let hub = hub.clone();
                std::thread::spawn(move || {
                    for _ in 0..200 {
                        hub.publish(Event::ObjectRemoved {
                            file_name: "gone.dobj".to_string(),
                        });
                    }
                })
            })
            .collect::<Vec<_>>();
        for publisher in publishers {
            publisher.join().unwrap();
        }
        for expected in 1..=800 {
            assert_eq!(rx.try_recv().unwrap().seq, expected);
        }
    }
}
