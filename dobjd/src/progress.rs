use driver::{ExecuteActionResult, ExecutionReporter, ExecutionStepContext};
use wire_types::{ExecutionPhase, ObjectStatus, ProofProgressStatus, RunActionProgress};

use crate::events::{Event, EventTx};

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

    fn send(&self, progress: RunActionProgress) {
        if let Err(err) = self.events.send(Event::RunActionProgress(progress)) {
            tracing::trace!(
                run_id = %self.run_id,
                "SSE broadcast dropped progress event: {err}",
            );
        }
    }

    /// Terminal failure event. The `ExecutionReporter` trait only fires
    /// `on_step` / `on_done` and never sees errors — `Driver::
    /// execute_with_reporter` just returns `Err`. Without this, SSE
    /// subscribers stay stuck in the last in-flight step and never receive
    /// a terminal `Done`/`Failed` to clear their progress UI. Callers
    /// invoke this from the same scope that owns the reporter, after
    /// `execute_with_reporter` errors.
    pub fn commit_failed(&self, message: impl Into<String>) {
        self.send(RunActionProgress {
            run_id: self.run_id.clone(),
            phase: ExecutionPhase::Commit,
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
        let progress = match phase {
            ExecutionPhase::GenerateProof => RunActionProgress {
                run_id: self.run_id.clone(),
                phase: ExecutionPhase::GenerateProof,
                status: ProofProgressStatus::Running,
                message: message.to_string(),
                old_root: None,
                new_root: None,
                output_files: None,
                output_status: None,
                nullified_files: None,
            },
            ExecutionPhase::Commit => RunActionProgress {
                run_id: self.run_id.clone(),
                phase: ExecutionPhase::Commit,
                status: ProofProgressStatus::Running,
                message: message.to_string(),
                old_root: ctx.old_root.clone(),
                new_root: None,
                output_files: (!ctx.output_files.is_empty()).then(|| ctx.output_files.clone()),
                output_status: ctx.output_status,
                nullified_files: None,
            },
        };
        self.send(progress);
    }

    fn on_done(&self, phase: ExecutionPhase, result: Option<&ExecuteActionResult>) {
        let progress = match phase {
            ExecutionPhase::GenerateProof => RunActionProgress {
                run_id: self.run_id.clone(),
                phase: ExecutionPhase::GenerateProof,
                status: ProofProgressStatus::Done,
                message: "Proof generation complete".to_string(),
                old_root: None,
                new_root: None,
                output_files: None,
                output_status: None,
                nullified_files: None,
            },
            ExecutionPhase::Commit => match result {
                Some(result) => RunActionProgress {
                    run_id: self.run_id.clone(),
                    phase: ExecutionPhase::Commit,
                    status: ProofProgressStatus::Done,
                    message: "Commit complete".to_string(),
                    old_root: Some(result.old_root.clone()),
                    new_root: Some(result.new_root.clone()),
                    output_files: Some(result.output_files.clone()),
                    output_status: Some(ObjectStatus::Live),
                    nullified_files: Some(result.nullified_files.clone()),
                },
                None => return,
            },
        };
        self.send(progress);
    }
}
