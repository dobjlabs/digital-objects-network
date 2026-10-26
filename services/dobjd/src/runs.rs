//! In-memory registry of action runs.
//!
//! `POST /actions/run` (and the MCP `run_action` tool) return immediately with
//! a run id. A run composes the transaction stages: it waits for a proof
//! permit, proves and records, submits, then follows its transaction record
//! until the chain decides. Progress and the terminal outcome go into a
//! [`RunEntry`] here, which backs `GET /actions/runs/{id}` and its SSE stream.
//!
//! Entries are kept in memory only. A terminal run is retained for
//! [`RUN_RETENTION`] so a disconnected client can still read its result, then
//! reaped. The work never depends on this state surviving: the transaction
//! record on disk is what settlement follows.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use driver::{Driver, ExecuteActionInput, ExecutionReporter, ExecutionStepContext};
use tokio::sync::Semaphore;
use wire_types::transactions::{TransactionRecord, TransactionState};
use wire_types::{
    ActionArgValues, ExecutionPhase, ObjectStatus, ObjectSummary, ProofProgressStatus,
    QualifiedName, RunAccepted, RunActionProgress, RunActionResult, RunState, RunStatus,
};

use crate::events::{Event, EventHub};
use crate::state::Services;

/// How long a terminal run is retained for polling/replay before reaping.
pub const RUN_RETENTION: Duration = Duration::from_secs(600);
/// How often the reaper sweeps for expired terminal runs and jobs.
pub const REAP_INTERVAL: Duration = Duration::from_secs(60);
/// Default concurrency limit for runs; excludes standalone proof jobs.
pub const DEFAULT_MAX_CONCURRENT_PROOFS: usize = 1;

/// Wall-clock time in Unix epoch milliseconds, for the timestamps clients
/// read. Reaping uses the monotonic `Instant` in `RunInner::finished_at`.
pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

struct RunInner {
    status: RunStatus,
    progress: Vec<RunActionProgress>,
    result: Option<RunActionResult>,
    error: Option<String>,
    /// Set when the run reached a terminal state; gates the reaper. The
    /// mutators assert it is `None` on entry, so updating a finished run is a
    /// loud bug rather than a silent no-op.
    finished_at: Option<Instant>,
    started_at_ms: Option<u64>,
    finished_at_ms: Option<u64>,
}

/// One run's mutable state plus its identity.
pub struct RunEntry {
    run_id: String,
    action: QualifiedName,
    input_object_paths: Vec<String>,
    created_at_ms: u64,
    /// Registration order; breaks ties between runs created in the same
    /// millisecond when listing oldest first.
    sequence: u64,
    inner: Mutex<RunInner>,
}

impl RunEntry {
    fn new(
        run_id: String,
        action: QualifiedName,
        input_object_paths: Vec<String>,
        sequence: u64,
    ) -> Self {
        Self {
            run_id,
            action,
            input_object_paths,
            created_at_ms: now_ms(),
            sequence,
            inner: Mutex::new(RunInner {
                status: RunStatus::Queued,
                progress: Vec::new(),
                result: None,
                error: None,
                finished_at: None,
                started_at_ms: None,
                finished_at_ms: None,
            }),
        }
    }

    /// Append progress while the run is active; reject updates after completion.
    fn push_progress(&self, progress: RunActionProgress) {
        let mut inner = self.inner.lock().unwrap();
        assert!(
            inner.finished_at.is_none(),
            "run state mutated after it finished"
        );
        inner.status = match progress.phase {
            ExecutionPhase::GenerateProof => RunStatus::GenerateProof,
            ExecutionPhase::Commit => RunStatus::Committing,
        };
        inner.started_at_ms.get_or_insert(progress.at_ms);
        inner.progress.push(progress);
    }

    fn succeed(&self, result: RunActionResult) {
        let mut inner = self.inner.lock().unwrap();
        assert!(
            inner.finished_at.is_none(),
            "run state mutated after it finished"
        );
        inner.status = RunStatus::Succeeded;
        inner.result = Some(result);
        inner.finished_at = Some(Instant::now());
        inner.finished_at_ms = Some(now_ms());
    }

    /// Record terminal failure: append a `Failed` event tagged with the phase
    /// the run was in then set the error + status. Returns the event so the caller
    /// can broadcast it too. Asserts the run hasn't already finished.
    fn fail(&self, message: String) -> RunActionProgress {
        let mut inner = self.inner.lock().unwrap();
        assert!(
            inner.finished_at.is_none(),
            "run state mutated after it finished"
        );
        // The current status is the phase the run reached; a pre-proof failure
        // (still `Queued`) is reported against `GenerateProof`.
        let phase = match inner.status {
            RunStatus::Committing => ExecutionPhase::Commit,
            _ => ExecutionPhase::GenerateProof,
        };
        let event = RunActionProgress {
            run_id: self.run_id.clone(),
            phase,
            status: ProofProgressStatus::Failed,
            message: message.clone(),
            old_root: None,
            output_files: None,
            output_status: None,
            input_files: None,
            at_ms: now_ms(),
        };
        inner.started_at_ms.get_or_insert(event.at_ms);
        inner.progress.push(event.clone());
        inner.status = RunStatus::Failed;
        inner.error = Some(message);
        inner.finished_at = Some(Instant::now());
        inner.finished_at_ms = Some(event.at_ms);
        event
    }

    /// Progress events at index >= `from` (paired with their index, which is
    /// the SSE event id), plus whether the run has reached a terminal state.
    pub fn events_from(&self, from: usize) -> (Vec<(usize, RunActionProgress)>, bool) {
        let inner = self.inner.lock().unwrap();
        let events = inner
            .progress
            .iter()
            .enumerate()
            .skip(from)
            .map(|(index, progress)| (index, progress.clone()))
            .collect();
        (events, inner.finished_at.is_some())
    }

    pub fn snapshot(&self) -> RunState {
        let inner = self.inner.lock().unwrap();
        RunState {
            run_id: self.run_id.clone(),
            action: self.action.clone(),
            status: inner.status,
            input_object_paths: self.input_object_paths.clone(),
            created_at_ms: self.created_at_ms,
            started_at_ms: inner.started_at_ms,
            finished_at_ms: inner.finished_at_ms,
            result: inner.result.clone(),
            error: inner.error.clone(),
            progress: inner.progress.clone(),
        }
    }

    fn is_finished(&self) -> bool {
        self.inner.lock().unwrap().finished_at.is_some()
    }

    fn expired(&self, ttl: Duration) -> bool {
        self.inner
            .lock()
            .unwrap()
            .finished_at
            .map(|at| at.elapsed() >= ttl)
            .unwrap_or(false)
    }
}

#[derive(Clone)]
pub struct RunRegistry {
    runs: Arc<RwLock<HashMap<String, Arc<RunEntry>>>>,
    /// Runs remain queued while waiting for a proof permit.
    proof_permits: Arc<Semaphore>,
    next_sequence: Arc<AtomicU64>,
}

impl Default for RunRegistry {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_CONCURRENT_PROOFS)
    }
}

impl RunRegistry {
    pub fn new(max_concurrent_proofs: usize) -> Self {
        Self {
            runs: Arc::default(),
            proof_permits: Arc::new(Semaphore::new(max_concurrent_proofs.max(1))),
            next_sequence: Arc::default(),
        }
    }

    /// Register a fresh run and return its entry. `run_id` is a daemon-minted
    /// UUID, so it never collides with an existing run.
    fn start(
        &self,
        run_id: String,
        action: QualifiedName,
        input_object_paths: Vec<String>,
    ) -> Arc<RunEntry> {
        let sequence = self.next_sequence.fetch_add(1, Ordering::Relaxed);
        let entry = Arc::new(RunEntry::new(
            run_id.clone(),
            action,
            input_object_paths,
            sequence,
        ));
        self.runs.write().unwrap().insert(run_id, entry.clone());
        entry
    }

    pub fn get(&self, run_id: &str) -> Option<Arc<RunEntry>> {
        self.runs.read().unwrap().get(run_id).cloned()
    }

    /// Every retained run, oldest first: in-flight runs plus terminal runs
    /// not yet reaped. `active_only` keeps just the in-flight ones.
    pub fn list(&self, active_only: bool) -> Vec<RunState> {
        let mut entries: Vec<Arc<RunEntry>> = self
            .runs
            .read()
            .unwrap()
            .values()
            .filter(|entry| !(active_only && entry.is_finished()))
            .cloned()
            .collect();
        entries.sort_by_key(|entry| entry.sequence);
        entries.iter().map(|entry| entry.snapshot()).collect()
    }

    /// Maps the basename of each `.dobj` file held as an input by a run that
    /// has not yet succeeded or failed to that run's id. A file given to more
    /// than one such run maps to the oldest of them.
    pub fn held_input_run_ids(&self) -> HashMap<String, String> {
        let mut in_flight: Vec<Arc<RunEntry>> = self
            .runs
            .read()
            .unwrap()
            .values()
            .filter(|entry| !entry.is_finished())
            .cloned()
            .collect();
        in_flight.sort_by_key(|entry| entry.sequence);
        let mut run_ids_by_file = HashMap::new();
        for entry in &in_flight {
            for path in &entry.input_object_paths {
                if let Some(file_name) = Path::new(path).file_name().and_then(|name| name.to_str())
                {
                    run_ids_by_file
                        .entry(file_name.to_string())
                        .or_insert_with(|| entry.run_id.clone());
                }
            }
        }
        run_ids_by_file
    }

    /// Drop terminal runs older than [`RUN_RETENTION`].
    pub fn reap(&self) {
        self.runs
            .write()
            .unwrap()
            .retain(|_, entry| !entry.expired(RUN_RETENTION));
    }
}

/// Basename to the id of the run or proof job holding it as an input: the
/// driver's hold table while a proof is in progress, the run registry while
/// a run follows its transaction to the chain.
pub fn held_labels(driver: &Driver, runs: &RunRegistry) -> HashMap<String, String> {
    let mut held = runs.held_input_run_ids();
    for (file_name, owner) in driver.held_inputs() {
        held.entry(file_name).or_insert(owner);
    }
    held
}

pub fn fill_held_by(driver: &Driver, runs: &RunRegistry, objects: &mut [ObjectSummary]) {
    let held = held_labels(driver, runs);
    for object in objects {
        object.held_by_run_id = held.get(&object.file_name).cloned();
    }
}

/// Publishes progress to the event hub and retains it for polling and replay.
#[derive(Clone)]
pub struct RunReporter {
    events: EventHub,
    entry: Arc<RunEntry>,
    run_id: String,
}

impl RunReporter {
    fn new(events: EventHub, entry: Arc<RunEntry>, run_id: String) -> Self {
        Self {
            events,
            entry,
            run_id,
        }
    }

    fn emit(&self, progress: RunActionProgress) {
        self.events
            .publish(Event::RunActionProgress(progress.clone()));
        self.entry.push_progress(progress);
    }

    fn commit_step(&self, message: &str, ctx: &ExecutionStepContext) {
        self.on_step(ExecutionPhase::Commit, message, ctx);
    }

    fn finish_landed(&self, record: &TransactionRecord) {
        tracing::info!(run_id = %self.run_id, tx_final = %record.tx_final, "Transaction landed");
        self.emit(RunActionProgress {
            run_id: self.run_id.clone(),
            phase: ExecutionPhase::Commit,
            status: ProofProgressStatus::Done,
            message: "Transaction landed".to_string(),
            old_root: Some(record.grounding_root.clone()),
            output_files: Some(record.output_files.clone()),
            output_status: Some(ObjectStatus::Live),
            input_files: Some(record.input_files.clone()),
            at_ms: now_ms(),
        });
        self.entry.succeed(RunActionResult {
            run_id: self.run_id.clone(),
            old_root: record.grounding_root.clone(),
            output_files: record.output_files.clone(),
            input_files: record.input_files.clone(),
        });
    }

    fn finish_failure(&self, message: String) {
        tracing::error!(run_id = %self.run_id, error = %message, "Action failed");
        let event = self.entry.fail(message);
        self.events.publish(Event::RunActionProgress(event));
    }
}

impl ExecutionReporter for RunReporter {
    fn on_step(&self, phase: ExecutionPhase, message: &str, ctx: &ExecutionStepContext) {
        tracing::info!(run_id = %self.run_id, ?phase, "{message}");
        let progress = match phase {
            ExecutionPhase::GenerateProof => RunActionProgress {
                run_id: self.run_id.clone(),
                phase,
                status: ProofProgressStatus::Running,
                message: message.to_string(),
                old_root: None,
                output_files: None,
                output_status: None,
                input_files: None,
                at_ms: now_ms(),
            },
            ExecutionPhase::Commit => RunActionProgress {
                run_id: self.run_id.clone(),
                phase,
                status: ProofProgressStatus::Running,
                message: message.to_string(),
                old_root: ctx.old_root.as_ref().map(payload::encode_hash_hex),
                output_files: (!ctx.output_files.is_empty()).then(|| ctx.output_files.clone()),
                output_status: ctx.output_status,
                input_files: None,
                at_ms: now_ms(),
            },
        };
        self.emit(progress);
    }

    fn on_done(&self, phase: ExecutionPhase, _result: Option<&driver::ExecuteActionResult>) {
        if phase == ExecutionPhase::GenerateProof {
            self.emit(RunActionProgress {
                run_id: self.run_id.clone(),
                phase,
                status: ProofProgressStatus::Done,
                message: "Proof generation complete".to_string(),
                old_root: None,
                output_files: None,
                output_status: None,
                input_files: None,
                at_ms: now_ms(),
            });
        }
    }
}

/// Start background execution and return a run handle.
pub fn spawn_run(
    services: &Services,
    action: QualifiedName,
    input_objects: Vec<String>,
    args: ActionArgValues,
) -> anyhow::Result<RunAccepted> {
    let exec_input = ExecuteActionInput {
        action: action.clone(),
        input_objects: input_objects.clone(),
        args,
    };
    let stage_services = services.clone();
    let read_services = services.clone();
    spawn_worker(
        &services.runs,
        services.events.clone(),
        action,
        input_objects,
        move |reporter| stage(&stage_services, exec_input, reporter),
        move |tx_final| {
            read_services
                .driver
                .get_transaction(&payload::decode_hash_hex(tx_final)?)
        },
    )
}

/// The blocking half of a run: prove and record under the run's id, then
/// submit. A relayer refusal is reported, not fatal: the transaction stays
/// recorded until it lands or expires, and the run follows it either way.
fn stage(
    services: &Services,
    input: ExecuteActionInput,
    reporter: &RunReporter,
) -> anyhow::Result<TransactionRecord> {
    let run_id = reporter.run_id.clone();
    let record = crate::jobs::prove_and_record(services, input, &run_id, reporter)?;
    let mut ctx = ExecutionStepContext {
        old_root: payload::decode_hash_hex(&record.grounding_root).ok(),
        output_files: record.output_files.clone(),
        output_status: Some(ObjectStatus::Unknown),
    };
    reporter.commit_step(
        "Transaction recorded; output object files created with status unknown",
        &ctx,
    );
    reporter.commit_step("Submitting proof to relayer", &ctx);
    let tx_final = payload::decode_hash_hex(&record.tx_final)?;
    let record = match services.driver.submit(&tx_final) {
        Ok(submitted) => {
            ctx.output_status = Some(ObjectStatus::Pending);
            reporter.commit_step(
                &format!(
                    "Relayer accepted job {}",
                    submitted.relayer_job_id.as_deref().unwrap_or("?")
                ),
                &ctx,
            );
            submitted
        }
        Err(err) => {
            reporter.commit_step(
                &format!(
                    "Relayer refused the proof: {err:#}; the transaction stays recorded until it lands or expires"
                ),
                &ctx,
            );
            services.driver.get_transaction(&tx_final)?
        }
    };
    services.announce_transaction(&record);
    reporter.commit_step("Waiting for the transaction to land", &ctx);
    Ok(record)
}

/// Register a run and drive it: wait for a proof permit, run `stage` on the
/// blocking pool, then follow the staged transaction's record through
/// `read_record` and the event hub until it is terminal.
fn spawn_worker<S, R>(
    registry: &RunRegistry,
    events: EventHub,
    action: QualifiedName,
    input_object_paths: Vec<String>,
    stage: S,
    read_record: R,
) -> anyhow::Result<RunAccepted>
where
    S: FnOnce(&RunReporter) -> anyhow::Result<TransactionRecord> + Send + 'static,
    R: Fn(&str) -> anyhow::Result<TransactionRecord> + Send + Sync + 'static,
{
    let run_id = uuid::Uuid::new_v4().to_string();
    let entry = registry.start(run_id.clone(), action, input_object_paths);
    let created_at_ms = entry.created_at_ms;
    let permits = registry.proof_permits.clone();
    let reporter = RunReporter::new(events.clone(), entry, run_id.clone());

    tokio::spawn(async move {
        let permit = permits
            .acquire_owned()
            .await
            .expect("proof semaphore is never closed");
        let worker = reporter.clone();
        let staged = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            stage(&worker)
        })
        .await;
        let record = match staged {
            Ok(Ok(record)) => record,
            Ok(Err(err)) => return reporter.finish_failure(format!("{err:#}")),
            Err(join_err) => {
                return reporter.finish_failure(format!("run worker panicked: {join_err}"));
            }
        };
        let tx_final = record.tx_final.clone();
        match follow_transaction(&events, &read_record, record).await {
            Ok(settled) => match settled.state {
                TransactionState::Landed => reporter.finish_landed(&settled),
                TransactionState::Expired => reporter.finish_failure(format!(
                    "transaction {} expired without landing: {}",
                    settled.tx_final,
                    settled.reason.as_deref().unwrap_or("grounding aged out")
                )),
                TransactionState::Recorded | TransactionState::Submitted => {
                    unreachable!("follow_transaction returns only terminal records")
                }
            },
            Err(err) => {
                reporter.finish_failure(format!("lost track of transaction {tx_final}: {err:#}"))
            }
        }
    });

    Ok(RunAccepted {
        run_id,
        status: RunStatus::Queued,
        created_at_ms,
    })
}

/// Returns the record once it is terminal. Subscribes before reading, so a
/// change between the two is seen either way.
async fn follow_transaction<R>(
    events: &EventHub,
    read_record: &R,
    staged: TransactionRecord,
) -> anyhow::Result<TransactionRecord>
where
    R: Fn(&str) -> anyhow::Result<TransactionRecord>,
{
    let mut rx = events.subscribe();
    let mut record = if staged.state.is_terminal() {
        staged
    } else {
        read_record(&staged.tx_final)?
    };
    loop {
        if record.state.is_terminal() {
            return Ok(record);
        }
        match rx.recv().await {
            Ok(envelope) => match envelope.event {
                Event::Transaction(changed) if changed.tx_final == record.tx_final => {
                    record = changed;
                }
                Event::TransactionDeleted { tx_final } if tx_final == record.tx_final => {
                    anyhow::bail!("transaction {tx_final} was deleted");
                }
                _ => {}
            },
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                record = read_record(&record.tx_final)?;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                anyhow::bail!("event hub closed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action(name: &str) -> QualifiedName {
        QualifiedName {
            plugin_name: "test".to_string(),
            name: name.to_string(),
        }
    }

    impl RunEntry {
        fn status(&self) -> RunStatus {
            self.inner.lock().unwrap().status
        }
    }

    fn step(
        phase: ExecutionPhase,
        status: ProofProgressStatus,
        message: &str,
    ) -> RunActionProgress {
        RunActionProgress {
            run_id: "r1".to_string(),
            phase,
            status,
            message: message.to_string(),
            old_root: None,
            output_files: None,
            output_status: None,
            input_files: None,
            at_ms: now_ms(),
        }
    }

    fn ok_result() -> RunActionResult {
        RunActionResult {
            run_id: "r1".to_string(),
            old_root: "0xold".to_string(),
            output_files: vec!["out.dobj".to_string()],
            input_files: vec!["in.dobj".to_string()],
        }
    }

    fn record(tx_final: &str, state: TransactionState) -> TransactionRecord {
        TransactionRecord {
            tx_final: tx_final.to_string(),
            action: action("A"),
            input_files: vec!["in.dobj".to_string()],
            output_files: vec!["out.dobj".to_string()],
            grounding_root: "0xroot".to_string(),
            grounding_block: 1,
            nullifiers: vec![],
            created: vec![],
            state,
            relayer_job_id: None,
            tx_hash: None,
            relayer_failure: None,
            reason: (state == TransactionState::Expired).then(|| "aged out".to_string()),
            recorded_at_ms: 0,
            submit_attempted_at_ms: None,
            submitted_at_ms: None,
            settled_at_ms: None,
        }
    }

    fn still_open(tx_final: &str) -> anyhow::Result<TransactionRecord> {
        Ok(record(tx_final, TransactionState::Submitted))
    }

    async fn wait_terminal(registry: &RunRegistry, run: &RunAccepted) -> RunState {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let state = registry.get(&run.run_id).unwrap().snapshot();
                if matches!(state.status, RunStatus::Succeeded | RunStatus::Failed) {
                    return state;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("run reached a terminal state")
    }

    async fn wait_status(registry: &RunRegistry, run: &RunAccepted, wanted: RunStatus) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while registry.get(&run.run_id).unwrap().status() != wanted {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("run did not reach {wanted:?}"));
    }

    #[tokio::test]
    async fn runs_queue_on_the_proof_permit_and_follow_their_transaction() {
        let registry = RunRegistry::new(1);
        let events = EventHub::new();
        let (first_started, first_start) = tokio::sync::oneshot::channel();
        let (release, released) = std::sync::mpsc::channel();
        let first = spawn_worker(
            &registry,
            events.clone(),
            action("first"),
            vec!["in.dobj".to_string()],
            move |reporter| {
                reporter.on_step(
                    ExecutionPhase::GenerateProof,
                    "Generating proof",
                    &Default::default(),
                );
                first_started.send(()).unwrap();
                released.recv_timeout(Duration::from_secs(5)).unwrap();
                reporter.commit_step("Waiting for the transaction to land", &Default::default());
                Ok(record("tx-1", TransactionState::Submitted))
            },
            still_open,
        )
        .unwrap();
        tokio::time::timeout(Duration::from_secs(5), first_start)
            .await
            .unwrap()
            .unwrap();

        let second = spawn_worker(
            &registry,
            events.clone(),
            action("second"),
            Vec::new(),
            |reporter| {
                reporter.commit_step("Waiting for the transaction to land", &Default::default());
                Ok(record("tx-2", TransactionState::Submitted))
            },
            still_open,
        )
        .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            registry.get(&second.run_id).unwrap().status(),
            RunStatus::Queued
        );
        assert_eq!(
            registry.held_input_run_ids(),
            HashMap::from([("in.dobj".to_string(), first.run_id.clone())])
        );

        release.send(()).unwrap();
        wait_status(&registry, &second, RunStatus::Committing).await;
        assert_eq!(
            registry.get(&first.run_id).unwrap().status(),
            RunStatus::Committing
        );
        events.publish(Event::Transaction(record("tx-2", TransactionState::Landed)));
        assert_eq!(
            wait_terminal(&registry, &second).await.status,
            RunStatus::Succeeded
        );
        assert_eq!(
            registry.get(&first.run_id).unwrap().status(),
            RunStatus::Committing
        );
        assert_eq!(
            registry.held_input_run_ids(),
            HashMap::from([("in.dobj".to_string(), first.run_id.clone())])
        );

        events.publish(Event::Transaction(record("tx-1", TransactionState::Landed)));
        let landed = wait_terminal(&registry, &first).await;
        assert_eq!(landed.status, RunStatus::Succeeded);
        let result = landed.result.unwrap();
        assert_eq!(result.output_files, vec!["out.dobj".to_string()]);
        assert_eq!(result.input_files, vec!["in.dobj".to_string()]);
        assert_eq!(result.old_root, "0xroot");
        let last = landed.progress.last().unwrap();
        assert_eq!(last.status, ProofProgressStatus::Done);
        assert_eq!(last.output_status, Some(ObjectStatus::Live));
        assert!(registry.held_input_run_ids().is_empty());
    }

    #[tokio::test]
    async fn a_run_fails_when_its_transaction_expires_or_disappears() {
        let registry = RunRegistry::new(1);
        let events = EventHub::new();
        let expired = spawn_worker(
            &registry,
            events.clone(),
            action("expiring"),
            Vec::new(),
            |reporter| {
                reporter.commit_step("Waiting", &Default::default());
                Ok(record("tx-1", TransactionState::Submitted))
            },
            still_open,
        )
        .unwrap();
        wait_status(&registry, &expired, RunStatus::Committing).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        events.publish(Event::Transaction(record(
            "tx-1",
            TransactionState::Expired,
        )));
        let state = wait_terminal(&registry, &expired).await;
        assert_eq!(state.status, RunStatus::Failed);
        assert!(state.error.unwrap().contains("aged out"));

        let deleted = spawn_worker(
            &registry,
            events.clone(),
            action("deleted"),
            Vec::new(),
            |reporter| {
                reporter.commit_step("Waiting", &Default::default());
                Ok(record("tx-2", TransactionState::Recorded))
            },
            still_open,
        )
        .unwrap();
        wait_status(&registry, &deleted, RunStatus::Committing).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        events.publish(Event::TransactionDeleted {
            tx_final: "tx-2".to_string(),
        });
        let state = wait_terminal(&registry, &deleted).await;
        assert_eq!(state.status, RunStatus::Failed);
        assert!(state.error.unwrap().contains("deleted"));

        let landed = spawn_worker(
            &registry,
            events,
            action("landed"),
            Vec::new(),
            |_| Ok(record("tx-3", TransactionState::Submitted)),
            |tx_final| Ok(record(tx_final, TransactionState::Landed)),
        )
        .unwrap();
        assert_eq!(
            wait_terminal(&registry, &landed).await.status,
            RunStatus::Succeeded
        );
    }

    #[tokio::test]
    async fn errors_and_panics_release_the_proof_permit() {
        let registry = RunRegistry::new(1);
        let events = EventHub::new();
        let refused = spawn_worker(
            &registry,
            events.clone(),
            action("refused"),
            Vec::new(),
            |_| Err(anyhow::anyhow!("input is held by another run")),
            still_open,
        )
        .unwrap();
        assert_eq!(
            wait_terminal(&registry, &refused).await.error.as_deref(),
            Some("input is held by another run")
        );
        let panicked = spawn_worker(
            &registry,
            events.clone(),
            action("panic"),
            Vec::new(),
            |_| {
                panic!("worker panic");
            },
            still_open,
        )
        .unwrap();
        let state = wait_terminal(&registry, &panicked).await;
        assert_eq!(state.status, RunStatus::Failed);
        assert!(state.error.unwrap().contains("worker panic"));
        let next = spawn_worker(
            &registry,
            events,
            action("next"),
            Vec::new(),
            |_| Ok(record("tx-9", TransactionState::Landed)),
            still_open,
        )
        .unwrap();
        assert_eq!(
            wait_terminal(&registry, &next).await.status,
            RunStatus::Succeeded
        );
    }

    #[test]
    fn progress_advances_status_and_indexes_events() {
        let entry = RunEntry::new("r1".to_string(), action("A"), Vec::new(), 0);
        assert_eq!(entry.status(), RunStatus::Queued);

        entry.push_progress(step(
            ExecutionPhase::GenerateProof,
            ProofProgressStatus::Running,
            "gen",
        ));
        assert_eq!(entry.status(), RunStatus::GenerateProof);
        entry.push_progress(step(
            ExecutionPhase::Commit,
            ProofProgressStatus::Running,
            "commit",
        ));
        assert_eq!(entry.status(), RunStatus::Committing);

        let (all, terminal) = entry.events_from(0);
        assert_eq!(
            all.iter().map(|(index, _)| *index).collect::<Vec<_>>(),
            [0, 1]
        );
        assert!(!terminal);

        // Replay from index 1 yields only the later event: the Last-Event-ID
        // resume semantics the SSE stream depends on.
        let (tail, _) = entry.events_from(1);
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].0, 1);
        assert_eq!(tail[0].1.message, "commit");
    }

    #[test]
    fn succeed_is_terminal() {
        let entry = RunEntry::new("r1".to_string(), action("A"), Vec::new(), 0);
        entry.push_progress(step(
            ExecutionPhase::GenerateProof,
            ProofProgressStatus::Running,
            "gen",
        ));
        entry.succeed(ok_result());

        let snapshot = entry.snapshot();
        assert_eq!(snapshot.status, RunStatus::Succeeded);
        assert_eq!(snapshot.result.unwrap().old_root, "0xold");
        assert!(snapshot.error.is_none());
        assert_eq!(snapshot.progress.len(), 1);
        let (_, terminal) = entry.events_from(0);
        assert!(terminal);
    }

    #[test]
    #[should_panic(expected = "mutated after it finished")]
    fn mutating_a_finished_run_panics() {
        let entry = RunEntry::new("r1".to_string(), action("A"), Vec::new(), 0);
        entry.succeed(ok_result());
        // A progress event after the run finished is a logic bug, not a no-op.
        entry.push_progress(step(
            ExecutionPhase::Commit,
            ProofProgressStatus::Running,
            "late",
        ));
    }

    #[test]
    fn fail_records_terminal_event_and_error() {
        let entry = RunEntry::new("r1".to_string(), action("A"), Vec::new(), 0);
        let event = entry.fail("boom".to_string());
        assert_eq!(event.status, ProofProgressStatus::Failed);

        let snapshot = entry.snapshot();
        assert_eq!(snapshot.status, RunStatus::Failed);
        assert_eq!(snapshot.error.as_deref(), Some("boom"));
        // The terminal failure event is appended so SSE subscribers see it.
        assert_eq!(snapshot.progress.len(), 1);
        assert_eq!(snapshot.progress[0].status, ProofProgressStatus::Failed);
    }

    #[test]
    fn fail_event_is_tagged_with_the_phase_the_run_was_in() {
        // No step has run yet -> attributed to proof generation, not commit.
        let queued = RunEntry::new("r1".to_string(), action("A"), Vec::new(), 0);
        assert_eq!(
            queued.fail("early".to_string()).phase,
            ExecutionPhase::GenerateProof
        );

        // Failing during proof generation -> GenerateProof.
        let proving = RunEntry::new("r2".to_string(), action("A"), Vec::new(), 0);
        proving.push_progress(step(
            ExecutionPhase::GenerateProof,
            ProofProgressStatus::Running,
            "proving",
        ));
        assert_eq!(
            proving.fail("proof failed".to_string()).phase,
            ExecutionPhase::GenerateProof
        );

        // Failing during commit -> Commit.
        let committing = RunEntry::new("r3".to_string(), action("A"), Vec::new(), 0);
        committing.push_progress(step(
            ExecutionPhase::Commit,
            ProofProgressStatus::Running,
            "submitting",
        ));
        assert_eq!(
            committing.fail("relayer rejected".to_string()).phase,
            ExecutionPhase::Commit
        );
    }

    #[test]
    fn expired_only_after_terminal() {
        let entry = RunEntry::new("r1".to_string(), action("A"), Vec::new(), 0);
        // In-flight runs are never reaped, even at zero TTL.
        assert!(!entry.expired(Duration::from_secs(0)));
        entry.succeed(ok_result());
        // Terminal + elapsed TTL => reapable; terminal within TTL => retained.
        assert!(entry.expired(Duration::from_secs(0)));
        assert!(!entry.expired(Duration::from_secs(3600)));
    }

    #[test]
    fn reaper_keeps_in_flight_and_recent_terminal_runs() {
        let registry = RunRegistry::default();
        let _live = registry.start("live".to_string(), action("A"), Vec::new());
        let done = registry.start("done".to_string(), action("A"), Vec::new());
        done.succeed(ok_result());
        // RUN_RETENTION has not elapsed, so nothing is dropped yet.
        registry.reap();
        assert!(registry.get("live").is_some());
        assert!(registry.get("done").is_some());
    }

    #[test]
    fn timestamps_follow_the_run_lifecycle() {
        let entry = RunEntry::new("r1".to_string(), action("A"), Vec::new(), 0);
        let queued = entry.snapshot();
        assert!(queued.created_at_ms > 0);
        assert_eq!(queued.started_at_ms, None);
        assert_eq!(queued.finished_at_ms, None);

        let first = step(
            ExecutionPhase::GenerateProof,
            ProofProgressStatus::Running,
            "gen",
        );
        let first_at_ms = first.at_ms;
        entry.push_progress(first);
        entry.push_progress(step(
            ExecutionPhase::Commit,
            ProofProgressStatus::Running,
            "commit",
        ));
        let running = entry.snapshot();
        assert_eq!(running.started_at_ms, Some(first_at_ms));
        assert_eq!(running.finished_at_ms, None);

        entry.succeed(ok_result());
        let finished = entry.snapshot().finished_at_ms.unwrap();
        assert!(finished >= first_at_ms);
    }

    #[test]
    fn failure_records_finish_time_even_when_queued() {
        let entry = RunEntry::new("r1".to_string(), action("A"), Vec::new(), 0);
        let event = entry.fail("boom".to_string());
        let snapshot = entry.snapshot();
        assert_eq!(snapshot.started_at_ms, Some(event.at_ms));
        assert_eq!(snapshot.finished_at_ms, Some(event.at_ms));
    }

    #[test]
    fn snapshot_reports_input_paths() {
        let inputs = vec!["a.dobj".to_string(), "b.dobj".to_string()];
        let entry = RunEntry::new("r1".to_string(), action("A"), inputs.clone(), 0);
        assert_eq!(entry.snapshot().input_object_paths, inputs);
    }

    #[test]
    fn list_is_oldest_first_and_can_keep_only_active_runs() {
        let registry = RunRegistry::default();
        for run_id in ["first", "second", "third"] {
            registry.start(run_id.to_string(), action("A"), Vec::new());
        }
        registry.get("second").unwrap().succeed(ok_result());

        let run_ids = |runs: Vec<RunState>| -> Vec<String> {
            runs.into_iter().map(|run| run.run_id).collect()
        };
        assert_eq!(run_ids(registry.list(false)), ["first", "second", "third"]);
        assert_eq!(run_ids(registry.list(true)), ["first", "third"]);
    }

    #[test]
    fn inputs_are_held_only_while_their_run_is_in_flight() {
        let registry = RunRegistry::default();
        let running = registry.start(
            "running".to_string(),
            action("A"),
            vec![
                "/elsewhere/held.dobj".to_string(),
                "also-held.dobj".to_string(),
            ],
        );
        let done = registry.start(
            "done".to_string(),
            action("A"),
            vec!["released.dobj".to_string()],
        );
        done.succeed(ok_result());
        assert_eq!(
            registry.held_input_run_ids(),
            HashMap::from([
                ("held.dobj".to_string(), "running".to_string()),
                ("also-held.dobj".to_string(), "running".to_string()),
            ])
        );
        running.fail("stopped".to_string());
        assert!(registry.held_input_run_ids().is_empty());
    }

    #[test]
    fn a_file_given_to_two_runs_is_held_by_the_older() {
        let registry = RunRegistry::default();
        registry.start(
            "older".to_string(),
            action("A"),
            vec!["shared.dobj".to_string()],
        );
        registry.start(
            "newer".to_string(),
            action("A"),
            vec!["shared.dobj".to_string()],
        );
        assert_eq!(
            registry.held_input_run_ids(),
            HashMap::from([("shared.dobj".to_string(), "older".to_string())])
        );
    }

    #[test]
    fn reporter_drives_entry_through_lifecycle() {
        let events = EventHub::new();
        let entry = Arc::new(RunEntry::new("r1".to_string(), action("A"), Vec::new(), 0));
        let reporter = RunReporter::new(events, entry.clone(), "r1".to_string());

        let ctx = ExecutionStepContext::default();
        reporter.on_step(ExecutionPhase::GenerateProof, "gen", &ctx);
        assert_eq!(entry.status(), RunStatus::GenerateProof);
        reporter.on_step(ExecutionPhase::Commit, "commit", &ctx);
        assert_eq!(entry.status(), RunStatus::Committing);

        reporter.finish_landed(&record("tx-1", TransactionState::Landed));

        let snapshot = entry.snapshot();
        assert_eq!(snapshot.status, RunStatus::Succeeded);
        let result = snapshot.result.unwrap();
        assert_eq!(result.run_id, "r1");
        assert_eq!(result.old_root, "0xroot");
        assert_eq!(result.input_files, vec!["in.dobj".to_string()]);
        assert_eq!(snapshot.progress.len(), 3);
    }

    #[test]
    fn reporter_failure_is_terminal_and_logged() {
        let events = EventHub::new();
        let entry = Arc::new(RunEntry::new("r1".to_string(), action("A"), Vec::new(), 0));
        let reporter = RunReporter::new(events, entry.clone(), "r1".to_string());

        reporter.finish_failure("kaboom".to_string());

        let snapshot = entry.snapshot();
        assert_eq!(snapshot.status, RunStatus::Failed);
        assert_eq!(snapshot.error.as_deref(), Some("kaboom"));
        assert!(
            snapshot
                .progress
                .iter()
                .any(|event| event.status == ProofProgressStatus::Failed)
        );
    }
}
