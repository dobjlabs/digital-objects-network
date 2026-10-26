//! In-memory proof jobs and the shared prove-and-record operation.
//! Completed transaction records persist independently of the job registry.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use driver::{ExecuteActionInput, ExecutionReporter, ExecutionStepContext};
use wire_types::transactions::TransactionRecord;
use wire_types::{ActionArgValues, ExecutionPhase, ProofJobState, ProofJobStatus, QualifiedName};

use crate::events::Event;
use crate::runs::now_ms;
use crate::state::Services;

/// How long a finished job is retained for polling before reaping.
pub const JOB_RETENTION: Duration = Duration::from_secs(600);

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
    job_id: String,
    action: QualifiedName,
    input_object_paths: Vec<String>,
    created_at_ms: u64,
    sequence: u64,
    inner: Mutex<JobInner>,
}

impl JobEntry {
    fn new(
        job_id: String,
        action: QualifiedName,
        input_object_paths: Vec<String>,
        sequence: u64,
    ) -> Self {
        Self {
            job_id,
            action,
            input_object_paths,
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
        }
    }

    pub fn snapshot(&self) -> ProofJobState {
        let inner = self.inner.lock().unwrap();
        ProofJobState {
            job_id: self.job_id.clone(),
            action: self.action.clone(),
            input_object_paths: self.input_object_paths.clone(),
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

    fn is_finished(&self) -> bool {
        self.inner.lock().unwrap().finished_at.is_some()
    }

    fn expired(&self, ttl: Duration) -> bool {
        self.inner
            .lock()
            .unwrap()
            .finished_at
            .is_some_and(|at| at.elapsed() >= ttl)
    }
}

#[derive(Clone, Default)]
pub struct JobRegistry {
    jobs: Arc<RwLock<HashMap<String, Arc<JobEntry>>>>,
    next_sequence: Arc<AtomicU64>,
}

impl JobRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    fn start(
        &self,
        job_id: String,
        action: QualifiedName,
        input_object_paths: Vec<String>,
    ) -> Arc<JobEntry> {
        let sequence = self.next_sequence.fetch_add(1, Ordering::Relaxed);
        let entry = Arc::new(JobEntry::new(
            job_id.clone(),
            action,
            input_object_paths,
            sequence,
        ));
        self.jobs.write().unwrap().insert(job_id, entry.clone());
        entry
    }

    pub fn get(&self, job_id: &str) -> Option<Arc<JobEntry>> {
        self.jobs.read().unwrap().get(job_id).cloned()
    }

    /// Every retained job, oldest first. `active_only` keeps the unfinished.
    pub fn list(&self, active_only: bool) -> Vec<ProofJobState> {
        let mut entries: Vec<Arc<JobEntry>> = self
            .jobs
            .read()
            .unwrap()
            .values()
            .filter(|entry| !(active_only && entry.is_finished()))
            .cloned()
            .collect();
        entries.sort_by_key(|entry| entry.sequence);
        entries.iter().map(|entry| entry.snapshot()).collect()
    }

    /// Drop finished jobs older than [`JOB_RETENTION`].
    pub fn reap(&self) {
        self.jobs
            .write()
            .unwrap()
            .retain(|_, entry| !entry.expired(JOB_RETENTION));
    }
}

struct JobReporter {
    services: Services,
    entry: Arc<JobEntry>,
}

impl JobReporter {
    fn progress(&self, status: ProofJobStatus, message: &str) {
        self.entry.progress(status, message);
        self.services
            .events
            .publish(Event::ProofJob(self.entry.snapshot()));
    }
}

impl ExecutionReporter for JobReporter {
    fn on_step(&self, phase: ExecutionPhase, message: &str, _ctx: &ExecutionStepContext) {
        let status = match phase {
            ExecutionPhase::GenerateProof => ProofJobStatus::Proving,
            ExecutionPhase::Commit => ProofJobStatus::Recording,
        };
        self.progress(status, message);
    }
}

/// Start a proof job and return before completion.
pub fn spawn_proof(
    services: &Services,
    action: QualifiedName,
    input_objects: Vec<String>,
    args: ActionArgValues,
) -> ProofJobState {
    let job_id = uuid::Uuid::new_v4().to_string();
    let entry = services
        .jobs
        .start(job_id.clone(), action.clone(), input_objects.clone());
    let accepted = entry.snapshot();
    services.events.publish(Event::ProofJob(accepted.clone()));

    let input = ExecuteActionInput {
        action,
        input_objects,
        args,
    };
    let reporter = Arc::new(JobReporter {
        services: services.clone(),
        entry,
    });
    tokio::spawn(async move {
        let worker = reporter.clone();
        let joined = tokio::task::spawn_blocking(move || {
            prove_and_record(&worker.services, input, &worker.entry.job_id, &*worker)
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

/// Prove and record, publishing transaction and object changes.
pub(crate) fn prove_and_record<R: ExecutionReporter>(
    services: &Services,
    input: ExecuteActionInput,
    owner: &str,
    reporter: &R,
) -> anyhow::Result<TransactionRecord> {
    let proved = services.driver.prove(input, owner, reporter)?;
    let input_files = proved.input_files.clone();
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
