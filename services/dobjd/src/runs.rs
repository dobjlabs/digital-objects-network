//! In-memory registry of action runs.
//!
//! `POST /actions/run` (and the MCP `run_action` tool) return immediately with
//! a run id; the proof + commit pipeline executes on a background task that
//! records progress and the terminal outcome into a [`RunEntry`] here. Clients
//! recover the outcome by polling `GET /actions/runs/{id}` or by streaming
//! `GET /actions/runs/{id}/events`, both of which read this state.
//!
//! Entries are kept in memory only. A terminal run is retained for
//! [`RUN_RETENTION`] so a disconnected client can still read its result, then
//! reaped. The work itself (and its on-chain / `.dobj` effects) never depends
//! on this state surviving -- `sync_objects` reconciles those independently.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use driver::{
    Driver, ExecuteActionInput, ExecuteActionResult, ExecutionReporter, ExecutionStepContext,
};
use wire_types::{
    ExecutionPhase, ObjectStatus, ProofProgressStatus, QualifiedName, RunAccepted,
    RunActionProgress, RunActionResult, RunState, RunStatus,
};

use crate::events::{Event, EventTx};

/// How long a terminal run is retained for polling/replay before reaping.
pub const RUN_RETENTION: Duration = Duration::from_secs(600);
/// How often the reaper sweeps for expired terminal runs.
pub const REAP_INTERVAL: Duration = Duration::from_secs(60);

struct RunInner {
    status: RunStatus,
    progress: Vec<RunActionProgress>,
    result: Option<RunActionResult>,
    error: Option<String>,
    /// Set when the run reached a terminal state; gates the reaper. The
    /// mutators assert it is `None` on entry, so updating a finished run is a
    /// loud bug rather than a silent no-op.
    finished_at: Option<Instant>,
}

/// One run's mutable state plus its identity.
pub struct RunEntry {
    run_id: String,
    action: QualifiedName,
    inner: Mutex<RunInner>,
}

impl RunEntry {
    fn new(run_id: String, action: QualifiedName) -> Self {
        Self {
            run_id,
            action,
            inner: Mutex::new(RunInner {
                status: RunStatus::Queued,
                progress: Vec::new(),
                result: None,
                error: None,
                finished_at: None,
            }),
        }
    }

    /// Append a non-terminal progress event and advance the status to match
    /// its phase. Asserts the run is still in flight — the worker never emits
    /// events after the run finishes.
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
            new_root: None,
            output_files: None,
            output_status: None,
            nullified_files: None,
        };
        inner.progress.push(event.clone());
        inner.status = RunStatus::Failed;
        inner.error = Some(message);
        inner.finished_at = Some(Instant::now());
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
            result: inner.result.clone(),
            error: inner.error.clone(),
            progress: inner.progress.clone(),
        }
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

#[derive(Clone, Default)]
pub struct RunRegistry {
    runs: Arc<RwLock<HashMap<String, Arc<RunEntry>>>>,
    execution_gate: Arc<tokio::sync::Mutex<()>>,
}

impl RunRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a fresh run and return its entry. `run_id` is a daemon-minted
    /// UUID, so it never collides with an existing run.
    fn start(&self, run_id: String, action: QualifiedName) -> Arc<RunEntry> {
        let entry = Arc::new(RunEntry::new(run_id.clone(), action));
        self.runs.write().unwrap().insert(run_id, entry.clone());
        entry
    }

    pub fn get(&self, run_id: &str) -> Option<Arc<RunEntry>> {
        self.runs.read().unwrap().get(run_id).cloned()
    }

    /// Drop terminal runs older than [`RUN_RETENTION`].
    pub fn reap(&self) {
        self.runs
            .write()
            .unwrap()
            .retain(|_, entry| !entry.expired(RUN_RETENTION));
    }
}

/// `ExecutionReporter` that fans every step out to both the global `/events`
/// broadcast (firehose subscribers) and the run's registry entry (which backs
/// `GET /actions/runs/{id}` and its SSE stream).
#[derive(Clone)]
struct RunReporter {
    events: EventTx,
    entry: Arc<RunEntry>,
    run_id: String,
}

impl RunReporter {
    fn new(events: EventTx, entry: Arc<RunEntry>, run_id: String) -> Self {
        Self {
            events,
            entry,
            run_id,
        }
    }

    fn emit(&self, progress: RunActionProgress) {
        // A broadcast send fails only when there are no subscribers, so ignore
        // the result; the registry entry is the durable record.
        let _ = self.events.send(Event::RunActionProgress(progress.clone()));
        self.entry.push_progress(progress);
    }

    fn finish_success(&self, result: &ExecuteActionResult) {
        self.entry.succeed(RunActionResult {
            run_id: self.run_id.clone(),
            old_root: payload::encode_hash_hex(&result.old_root),
            new_root: payload::encode_hash_hex(&result.new_root),
            output_files: result.output_files.clone(),
            nullified_files: result.nullified_files.clone(),
        });
    }

    fn finish_failure(&self, message: String) {
        // `fail` builds the event (tagged with the failing phase) and records
        // it in the registry; mirror it to the global hub.
        let event = self.entry.fail(message);
        let _ = self.events.send(Event::RunActionProgress(event));
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
                new_root: None,
                output_files: None,
                output_status: None,
                nullified_files: None,
            },
            ExecutionPhase::Commit => RunActionProgress {
                run_id: self.run_id.clone(),
                phase,
                status: ProofProgressStatus::Running,
                message: message.to_string(),
                old_root: ctx.old_root.as_ref().map(payload::encode_hash_hex),
                new_root: None,
                output_files: (!ctx.output_files.is_empty()).then(|| ctx.output_files.clone()),
                output_status: ctx.output_status,
                nullified_files: None,
            },
        };
        self.emit(progress);
    }

    fn on_done(&self, phase: ExecutionPhase, result: Option<&ExecuteActionResult>) {
        let progress = match phase {
            ExecutionPhase::GenerateProof => RunActionProgress {
                run_id: self.run_id.clone(),
                phase,
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
                    phase,
                    status: ProofProgressStatus::Done,
                    message: "Commit complete".to_string(),
                    old_root: Some(payload::encode_hash_hex(&result.old_root)),
                    new_root: Some(payload::encode_hash_hex(&result.new_root)),
                    output_files: Some(result.output_files.clone()),
                    output_status: Some(ObjectStatus::Live),
                    nullified_files: Some(result.nullified_files.clone()),
                },
                None => return,
            },
        };
        tracing::info!(run_id = %self.run_id, ?phase, "{}", progress.message);
        self.emit(progress);
    }
}

/// Mint a fresh run id, register it, and spawn the background worker that runs
/// the action and records its outcome. Returns immediately with the run handle;
/// follow the run via `GET /actions/runs/{run_id}` or its SSE stream.
pub fn spawn_run(
    registry: &RunRegistry,
    driver: Arc<Driver>,
    events: EventTx,
    action: QualifiedName,
    input_objects: Vec<String>,
) -> anyhow::Result<RunAccepted> {
    let exec_input = ExecuteActionInput {
        action: action.clone(),
        input_objects,
    };
    spawn_worker(registry, events, action, move |reporter| {
        driver.execute_with_reporter(exec_input, reporter)
    })
}

fn spawn_worker<F>(
    registry: &RunRegistry,
    events: EventTx,
    action: QualifiedName,
    execute: F,
) -> anyhow::Result<RunAccepted>
where
    F: FnOnce(&RunReporter) -> anyhow::Result<ExecuteActionResult> + Send + 'static,
{
    let permit = registry
        .execution_gate
        .clone()
        .try_lock_owned()
        .map_err(|_| driver::DriverError::Conflict("another action is already running".into()))?;
    let run_id = uuid::Uuid::new_v4().to_string();
    let entry = registry.start(run_id.clone(), action);

    let reporter = RunReporter::new(events, entry, run_id.clone());

    // A supervisor task drives the blocking pipeline and records the terminal
    // state on every exit path -- including a panic -- so a run can never get
    // stuck non-terminal. spawn_blocking keeps the CPU-bound proof off the
    // async runtime's worker threads.
    tokio::spawn(async move {
        let worker = reporter.clone();
        let join = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            execute(&worker)
        });
        match join.await {
            Ok(Ok(result)) => reporter.finish_success(&result),
            Ok(Err(err)) => reporter.finish_failure(format!("{err:#}")),
            Err(join_err) => reporter.finish_failure(format!("run worker panicked: {join_err}")),
        }
    });

    Ok(RunAccepted {
        run_id,
        status: RunStatus::Queued,
    })
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

    // Test-only accessor. `mod tests` is a child of `runs`, so it can both
    // add an inherent method to `RunEntry` and read its private `inner` —
    // keeping this out of the production impl where nothing else needs it.
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
            new_root: None,
            output_files: None,
            output_status: None,
            nullified_files: None,
        }
    }

    fn ok_result() -> RunActionResult {
        RunActionResult {
            run_id: "r1".to_string(),
            old_root: "0xold".to_string(),
            new_root: "0xnew".to_string(),
            output_files: vec!["out.dobj".to_string()],
            nullified_files: vec!["in.dobj".to_string()],
        }
    }

    fn execution_result() -> ExecuteActionResult {
        let root = payload::decode_hash_hex(
            "0000000000000000000000000000000000000000000000000000000000000001",
        )
        .unwrap();
        ExecuteActionResult {
            old_root: root,
            new_root: root,
            output_files: vec![],
            nullified_files: vec![],
            relayer_job_id: "job".into(),
            tx_hash: None,
            block_number: None,
        }
    }

    async fn wait_terminal(registry: &RunRegistry, run: &RunAccepted) -> RunState {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let state = registry.get(&run.run_id).unwrap().snapshot();
                if matches!(state.status, RunStatus::Succeeded | RunStatus::Failed) {
                    return state;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("run should finish")
    }

    #[tokio::test]
    async fn concurrent_runs_are_rejected_through_synchronizer_wait() {
        let registry = RunRegistry::new();
        let (events, _receiver) = crate::events::channel();
        let (first_started, first_start) = tokio::sync::oneshot::channel();
        let (confirm, confirmed) = std::sync::mpsc::channel();
        let first = spawn_worker(
            &registry,
            events.clone(),
            action("first"),
            move |reporter| {
                reporter.on_step(
                    ExecutionPhase::Commit,
                    "Waiting for synchronizer",
                    &Default::default(),
                );
                first_started.send(()).unwrap();
                confirmed.recv_timeout(Duration::from_secs(5)).unwrap();
                Ok(execution_result())
            },
        )
        .unwrap();
        let rejected = spawn_worker(&registry.clone(), events.clone(), action("early"), |_| {
            panic!("rejected action must not execute");
        })
        .unwrap_err();
        assert!(matches!(
            rejected.downcast_ref::<driver::DriverError>(),
            Some(driver::DriverError::Conflict(_))
        ));
        tokio::time::timeout(Duration::from_secs(5), first_start)
            .await
            .unwrap()
            .unwrap();

        let rejected = spawn_worker(&registry.clone(), events.clone(), action("second"), |_| {
            panic!("rejected action must not execute");
        })
        .unwrap_err();
        assert!(matches!(
            rejected.downcast_ref::<driver::DriverError>(),
            Some(driver::DriverError::Conflict(_))
        ));
        assert_eq!(
            axum::response::IntoResponse::into_response(crate::error::ApiError::from(rejected))
                .status(),
            axum::http::StatusCode::CONFLICT
        );
        assert_eq!(
            registry.get(&first.run_id).unwrap().status(),
            RunStatus::Committing
        );
        assert_eq!(registry.runs.read().unwrap().len(), 1);

        confirm.send(()).unwrap();
        assert_eq!(
            wait_terminal(&registry, &first).await.status,
            RunStatus::Succeeded
        );
        let second = spawn_worker(&registry, events, action("second"), |_| {
            Ok(execution_result())
        })
        .unwrap();
        assert_eq!(
            wait_terminal(&registry, &second).await.status,
            RunStatus::Succeeded
        );
    }

    #[tokio::test]
    async fn errors_and_panics_release_the_execution_gate() {
        let registry = RunRegistry::new();
        let (events, _receiver) = crate::events::channel();
        let timed_out = spawn_worker(&registry, events.clone(), action("timeout"), |_| {
            Err(anyhow::anyhow!("synchronizer timeout"))
        })
        .unwrap();
        assert_eq!(
            wait_terminal(&registry, &timed_out).await.error.as_deref(),
            Some("synchronizer timeout")
        );
        let panicked = spawn_worker(&registry, events.clone(), action("panic"), |_| {
            panic!("worker panic");
        })
        .unwrap();
        let state = wait_terminal(&registry, &panicked).await;
        assert_eq!(state.status, RunStatus::Failed);
        assert!(state.error.unwrap().contains("worker panic"));
        let next = spawn_worker(
            &registry,
            events,
            action("next"),
            |_| Ok(execution_result()),
        )
        .unwrap();
        assert_eq!(
            wait_terminal(&registry, &next).await.status,
            RunStatus::Succeeded
        );
    }

    #[test]
    fn progress_advances_status_and_indexes_events() {
        let entry = RunEntry::new("r1".to_string(), action("A"));
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

        // Replay from index 1 yields only the later event -- the Last-Event-ID
        // resume semantics the SSE stream depends on.
        let (tail, _) = entry.events_from(1);
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].0, 1);
        assert_eq!(tail[0].1.message, "commit");
    }

    #[test]
    fn succeed_is_terminal() {
        let entry = RunEntry::new("r1".to_string(), action("A"));
        entry.push_progress(step(
            ExecutionPhase::GenerateProof,
            ProofProgressStatus::Running,
            "gen",
        ));
        entry.succeed(ok_result());

        let snapshot = entry.snapshot();
        assert_eq!(snapshot.status, RunStatus::Succeeded);
        assert_eq!(snapshot.result.unwrap().new_root, "0xnew");
        assert!(snapshot.error.is_none());
        assert_eq!(snapshot.progress.len(), 1);
        let (_, terminal) = entry.events_from(0);
        assert!(terminal);
    }

    #[test]
    #[should_panic(expected = "mutated after it finished")]
    fn mutating_a_finished_run_panics() {
        let entry = RunEntry::new("r1".to_string(), action("A"));
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
        let entry = RunEntry::new("r1".to_string(), action("A"));
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
        let queued = RunEntry::new("r1".to_string(), action("A"));
        assert_eq!(
            queued.fail("early".to_string()).phase,
            ExecutionPhase::GenerateProof
        );

        // Failing during proof generation -> GenerateProof.
        let proving = RunEntry::new("r2".to_string(), action("A"));
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
        let committing = RunEntry::new("r3".to_string(), action("A"));
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
        let entry = RunEntry::new("r1".to_string(), action("A"));
        // In-flight runs are never reaped, even at zero TTL.
        assert!(!entry.expired(Duration::from_secs(0)));
        entry.succeed(ok_result());
        // Terminal + elapsed TTL => reapable; terminal within TTL => retained.
        assert!(entry.expired(Duration::from_secs(0)));
        assert!(!entry.expired(Duration::from_secs(3600)));
    }

    #[test]
    fn reaper_keeps_in_flight_and_recent_terminal_runs() {
        let registry = RunRegistry::new();
        let _live = registry.start("live".to_string(), action("A"));
        let done = registry.start("done".to_string(), action("A"));
        done.succeed(ok_result());
        // RUN_RETENTION has not elapsed, so nothing is dropped yet.
        registry.reap();
        assert!(registry.get("live").is_some());
        assert!(registry.get("done").is_some());
    }

    #[test]
    fn reporter_drives_entry_through_lifecycle() {
        let (events, _rx) = crate::events::channel();
        let entry = Arc::new(RunEntry::new("r1".to_string(), action("A")));
        let reporter = RunReporter::new(events, entry.clone(), "r1".to_string());

        let ctx = ExecutionStepContext::default();
        reporter.on_step(ExecutionPhase::GenerateProof, "gen", &ctx);
        assert_eq!(entry.status(), RunStatus::GenerateProof);
        reporter.on_step(ExecutionPhase::Commit, "commit", &ctx);
        assert_eq!(entry.status(), RunStatus::Committing);

        let old_root = payload::decode_hash_hex(
            "0000000000000000000000000000000000000000000000000000000000000001",
        )
        .unwrap();
        let new_root = payload::decode_hash_hex(
            "0000000000000000000000000000000000000000000000000000000000000002",
        )
        .unwrap();
        reporter.finish_success(&ExecuteActionResult {
            old_root,
            new_root,
            output_files: vec!["o.dobj".to_string()],
            nullified_files: vec!["n.dobj".to_string()],
            relayer_job_id: "job".to_string(),
            tx_hash: Some("0xtx".to_string()),
            block_number: Some(1),
        });

        let snapshot = entry.snapshot();
        assert_eq!(snapshot.status, RunStatus::Succeeded);
        let result = snapshot.result.unwrap();
        assert_eq!(result.run_id, "r1");
        assert_eq!(result.new_root, payload::encode_hash_hex(&new_root));
        assert_eq!(result.nullified_files, vec!["n.dobj".to_string()]);
    }

    #[test]
    fn reporter_failure_is_terminal_and_logged() {
        let (events, _rx) = crate::events::channel();
        let entry = Arc::new(RunEntry::new("r1".to_string(), action("A")));
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
