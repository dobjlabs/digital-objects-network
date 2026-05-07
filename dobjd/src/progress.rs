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
    Failed,
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

    /// Terminal failure event. The `ExecutionReporter` trait only fires
    /// `on_step` / `on_done` and never sees errors — `Driver::
    /// execute_with_reporter` just returns `Err`. Without this, SSE
    /// subscribers stay stuck in the last in-flight step and never receive
    /// a terminal `Done`/`Failed` to clear their progress UI. Callers
    /// invoke this from the same scope that owns the reporter, after
    /// `execute_with_reporter` errors.
    pub fn commit_failed(&self, message: impl Into<String>) {
        self.send(Event::RunActionProgress {
            run_id: self.run_id.clone(),
            phase: ProofPhase::Commit,
            status: ProofProgressStatus::Failed,
            message: message.into(),
            old_root: None,
            new_root: None,
            output_files: None,
            output_status: None,
            nullified_files: None,
        });
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
                output_status: None,
                nullified_files: None,
            },
            ExecutionPhase::Commit => Event::RunActionProgress {
                run_id: self.run_id.clone(),
                phase: ProofPhase::Commit,
                status: ProofProgressStatus::Running,
                message: message.to_string(),
                old_root: ctx.old_root.clone(),
                new_root: None,
                output_files: (!ctx.output_files.is_empty()).then(|| ctx.output_files.clone()),
                output_status: ctx.output_status,
                nullified_files: None,
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
                output_status: None,
                nullified_files: None,
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
                    output_status: Some(driver::ObjectStatus::Live),
                    nullified_files: Some(result.nullified_files.clone()),
                },
                None => return,
            },
        };
        self.send(event);
    }
}
