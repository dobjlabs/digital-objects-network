//! In-memory proof jobs and the shared prove-and-record operation.
//! Completed transaction records persist independently of the job registry.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use driver::{ExecuteActionInput, ExecutionReporter, ExecutionStepContext, now_ms};
use wire_types::transactions::TransactionRecord;
use wire_types::{ExecutionPhase, ProofJobState, ProofJobStatus, RunActionInput};

use crate::events::Event;
use crate::registry::{Registry, Retained};
use crate::state::Services;

pub type JobRegistry = Registry<JobEntry>;

struct JobInner {
    status: ProofJobStatus,
    message: Option<String>,
    tx_final: Option<String>,
    error: Option<String>,
    started_at_ms: Option<u64>,
    finished_at_ms: Option<u64>,
    finished_at: Option<Instant>,
}

pub struct JobEntry {
    input: RunActionInput,
    job_id: String,
    created_at_ms: u64,
    sequence: u64,
    inner: Mutex<JobInner>,
}

impl Retained for JobEntry {
    fn sequence(&self) -> u64 {
        self.sequence
    }

    fn finished_at(&self) -> Option<Instant> {
        self.inner.lock().unwrap().finished_at
    }
}

impl JobEntry {
    pub fn snapshot(&self) -> ProofJobState {
        let inner = self.inner.lock().unwrap();
        ProofJobState {
            job_id: self.job_id.clone(),
            action: self.input.action.clone(),
            input_object_paths: self.input.input_object_paths.clone(),
            status: inner.status,
            message: inner.message.clone(),
            tx_final: inner.tx_final.clone(),
            error: inner.error.clone(),
            created_at_ms: self.created_at_ms,
            started_at_ms: inner.started_at_ms,
            finished_at_ms: inner.finished_at_ms,
        }
    }

    fn progress(&self, status: ProofJobStatus, message: &str) {
        let mut inner = self.inner.lock().unwrap();
        assert!(
            inner.finished_at.is_none(),
            "job state mutated after it finished"
        );
        inner.status = status;
        inner.message = Some(message.to_string());
        inner.started_at_ms.get_or_insert_with(now_ms);
    }

    fn finish(&self, outcome: Result<&TransactionRecord, String>) {
        let mut inner = self.inner.lock().unwrap();
        assert!(
            inner.finished_at.is_none(),
            "job state mutated after it finished"
        );
        match outcome {
            Ok(record) => {
                inner.status = ProofJobStatus::Done;
                inner.tx_final = Some(record.tx_final.clone());
                inner.message = Some("Transaction recorded".to_string());
            }
            Err(error) => {
                inner.status = ProofJobStatus::Failed;
                inner.error = Some(error);
            }
        }
        let at_ms = now_ms();
        inner.started_at_ms.get_or_insert(at_ms);
        inner.finished_at_ms = Some(at_ms);
        inner.finished_at = Some(Instant::now());
    }
}

struct JobReporter {
    services: Services,
    entry: Arc<JobEntry>,
}

impl ExecutionReporter for JobReporter {
    fn on_step(&self, phase: ExecutionPhase, message: &str, _ctx: &ExecutionStepContext) {
        let status = match phase {
            ExecutionPhase::GenerateProof => ProofJobStatus::Proving,
            ExecutionPhase::Commit => ProofJobStatus::Recording,
        };
        self.entry.progress(status, message);
        self.services
            .events
            .publish(Event::ProofJob(self.entry.snapshot()));
    }
}

/// Start a proof job and return before completion.
pub fn spawn_proof(services: &Services, input: RunActionInput) -> ProofJobState {
    let input = trimmed(input);
    let job_id = uuid::Uuid::new_v4().to_string();
    let entry = services.jobs.start(job_id.clone(), |sequence| JobEntry {
        input: input.clone(),
        job_id,
        created_at_ms: now_ms(),
        sequence,
        inner: Mutex::new(JobInner {
            status: ProofJobStatus::Queued,
            message: None,
            tx_final: None,
            error: None,
            started_at_ms: None,
            finished_at_ms: None,
            finished_at: None,
        }),
    });
    let accepted = entry.snapshot();
    services.events.publish(Event::ProofJob(accepted.clone()));

    let reporter = Arc::new(JobReporter {
        services: services.clone(),
        entry,
    });
    tokio::spawn(async move {
        let worker = reporter.clone();
        let joined = tokio::task::spawn_blocking(move || {
            prove_and_record(
                &worker.services,
                execute_input(&worker.entry.input),
                &worker.entry.job_id,
                &*worker,
            )
        })
        .await;
        let outcome = match &joined {
            Ok(Ok(record)) => Ok(record),
            Ok(Err(err)) => Err(format!("{err:#}")),
            Err(join_err) => Err(format!("proof worker panicked: {join_err}")),
        };
        if let Err(error) = &outcome {
            tracing::error!(job_id = %reporter.entry.job_id, error = %error, "Proof job failed");
        }
        reporter.entry.finish(outcome);
        reporter
            .services
            .events
            .publish(Event::ProofJob(reporter.entry.snapshot()));
    });
    accepted
}

/// Trim whitespace without changing the paths supplied by the caller.
pub(crate) fn trimmed(mut input: RunActionInput) -> RunActionInput {
    for path in &mut input.input_object_paths {
        *path = path.trim().to_string();
    }
    input
}

pub(crate) fn execute_input(input: &RunActionInput) -> ExecuteActionInput {
    ExecuteActionInput {
        action: input.action.clone(),
        input_objects: input.input_object_paths.clone(),
        args: input.args.clone(),
    }
}

/// Prove and record, publishing transaction and object changes.
pub(crate) fn prove_and_record<R: ExecutionReporter>(
    services: &Services,
    input: ExecuteActionInput,
    owner: &str,
    reporter: &R,
) -> anyhow::Result<TransactionRecord> {
    let proved = services.driver.prove(input, owner, reporter)?;
    let input_files = proved.input_files().to_vec();
    services.publish_objects(&input_files);
    let recorded = services.driver.record(proved);
    match recorded {
        Ok(record) => {
            services.announce_transaction(&record);
            Ok(record)
        }
        Err(err) => {
            services.publish_objects(&input_files);
            Err(err)
        }
    }
}
