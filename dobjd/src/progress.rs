use serde::Serialize;

use driver::{ExecuteActionResult, ExecutionPhase, ExecutionReporter, ExecutionStepContext};

use crate::events::{Event, EventTx};

#[derive(Debug, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ProofPhase {
    GenerateProof,
    Commit,
}

#[derive(Debug, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ProofProgressStatus {
    Running,
    Done,
}

/// `ExecutionReporter` impl that broadcasts every step over the SSE event
/// hub.
pub struct SseProgressReporter {
    events: EventTx,
    run_id: String,
}

impl SseProgressReporter {
    pub fn new(events: EventTx, run_id: String) -> Self {
        Self { events, run_id }
    }

    fn send(&self, event: Event) {
        let _ = self.events.send(event);
    }
}

impl ExecutionReporter for SseProgressReporter {
    fn on_step(&self, phase: ExecutionPhase, message: &str, ctx: &ExecutionStepContext) {
        let event = match phase {
            ExecutionPhase::GenerateProof => Event::RunActionProgress {
                run_id: self.run_id.clone(),
                phase: ProofPhase::GenerateProof,
                status: ProofProgressStatus::Running,
                message: message.to_string(),
                old_root: None,
                new_root: None,
                output_files: None,
            },
            ExecutionPhase::Commit => Event::RunActionProgress {
                run_id: self.run_id.clone(),
                phase: ProofPhase::Commit,
                status: ProofProgressStatus::Running,
                message: message.to_string(),
                old_root: ctx.old_root.clone(),
                new_root: None,
                output_files: None,
            },
        };
        self.send(event);
    }

    fn on_done(&self, phase: ExecutionPhase, result: Option<&ExecuteActionResult>) {
        let event = match phase {
            ExecutionPhase::GenerateProof => Event::RunActionProgress {
                run_id: self.run_id.clone(),
                phase: ProofPhase::GenerateProof,
                status: ProofProgressStatus::Done,
                message: "Proof generation complete".to_string(),
                old_root: None,
                new_root: None,
                output_files: None,
            },
            ExecutionPhase::Commit => match result {
                Some(result) => Event::RunActionProgress {
                    run_id: self.run_id.clone(),
                    phase: ProofPhase::Commit,
                    status: ProofProgressStatus::Done,
                    message: "Commit complete".to_string(),
                    old_root: Some(result.old_root.clone()),
                    new_root: Some(result.new_root.clone()),
                    output_files: Some(result.output_files.clone()),
                },
                None => return,
            },
        };
        self.send(event);
    }
}
