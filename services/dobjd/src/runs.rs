//! In-memory run progress and results for polling and SSE replay.
//! Runs prove, record, submit, and follow settlement. Transaction records
//! persist independently, so settlement continues after the run registry is lost.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use driver::{ExecutionReporter, ExecutionStepContext, now_ms};
use wire_types::transactions::{TransactionRecord, TransactionState};
use wire_types::{
    ActionInput, ExecutionPhase, ExecutionStage, ObjectStatus, ProofProgressStatus, QualifiedName,
    RunAccepted, RunActionProgress, RunActionResult, RunStageTiming, RunState, RunStatus,
};

use crate::events::{Event, EventHub};
use crate::registry::{Registry, Retained};
use crate::state::{Services, transaction_event};

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
    stage_timings: Vec<RunStageTiming>,
    stage_started: Option<Instant>,
}

impl RunInner {
    fn finish_stage(&mut self, status: ProofProgressStatus, now: Instant) {
        if let Some(started) = self.stage_started.take() {
            let timing = self.stage_timings.last_mut().unwrap();
            timing.elapsed_ms = now.duration_since(started).as_millis() as u64;
            timing.status = status;
        }
    }

    fn begin_stage(&mut self, stage: ExecutionStage, now: Instant, at_ms: u64) {
        if self.stage_started.is_some()
            && self
                .stage_timings
                .last()
                .is_some_and(|timing| timing.stage == stage)
        {
            return;
        }
        self.finish_stage(ProofProgressStatus::Done, now);
        self.stage_timings.push(RunStageTiming {
            stage,
            status: ProofProgressStatus::Running,
            started_at_ms: at_ms,
            elapsed_ms: 0,
        });
        self.stage_started = Some(now);
    }

    fn timings_at(&self, now: Instant) -> Vec<RunStageTiming> {
        let mut timings = self.stage_timings.clone();
        if let Some(started) = self.stage_started {
            timings.last_mut().unwrap().elapsed_ms = now.duration_since(started).as_millis() as u64;
        }
        timings
    }
}

/// One run's mutable state plus its identity.
pub struct RunEntry {
    run_id: String,
    action: QualifiedName,
    input_object_paths: Vec<String>,
    created_at_ms: u64,
    created_at: Instant,
    sequence: u64,
    inner: Mutex<RunInner>,
}

impl Retained for RunEntry {
    fn sequence(&self) -> u64 {
        self.sequence
    }

    fn finished_at(&self) -> Option<Instant> {
        self.inner.lock().unwrap().finished_at
    }
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
            created_at: Instant::now(),
            sequence,
            inner: Mutex::new(RunInner {
                status: RunStatus::Queued,
                progress: Vec::new(),
                result: None,
                error: None,
                finished_at: None,
                started_at_ms: None,
                finished_at_ms: None,
                stage_timings: Vec::new(),
                stage_started: None,
            }),
        }
    }

    /// Append a non-terminal progress event and advance the status to match
    /// its phase. Asserts the run is still in flight -- the worker never emits
    /// events after the run finishes.
    fn push_progress(&self, mut progress: RunActionProgress) -> RunActionProgress {
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
        let now = Instant::now();
        progress.elapsed_ms = now.duration_since(self.created_at).as_millis() as u64;
        if let Some(stage) = progress.stage {
            inner.begin_stage(stage, now, progress.at_ms);
        }
        if progress.status != ProofProgressStatus::Running {
            inner.finish_stage(progress.status, now);
        }
        inner.progress.push(progress.clone());
        progress
    }

    fn succeed(&self, result: RunActionResult) {
        let mut inner = self.inner.lock().unwrap();
        assert!(
            inner.finished_at.is_none(),
            "run state mutated after it finished"
        );
        inner.status = RunStatus::Succeeded;
        inner.result = Some(result);
        let now = Instant::now();
        inner.finish_stage(ProofProgressStatus::Done, now);
        inner.finished_at = Some(now);
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
        let now = Instant::now();
        let stage = inner
            .stage_started
            .and_then(|_| inner.stage_timings.last().map(|timing| timing.stage));
        inner.finish_stage(ProofProgressStatus::Failed, now);
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
            stage,
            elapsed_ms: now.duration_since(self.created_at).as_millis() as u64,
        };
        inner.started_at_ms.get_or_insert(event.at_ms);
        inner.progress.push(event.clone());
        inner.status = RunStatus::Failed;
        inner.error = Some(message);
        inner.finished_at = Some(now);
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
        let now = inner.finished_at.unwrap_or_else(Instant::now);
        RunState {
            run_id: self.run_id.clone(),
            action: self.action.clone(),
            status: inner.status,
            input_object_paths: self.input_object_paths.clone(),
            created_at_ms: self.created_at_ms,
            started_at_ms: inner.started_at_ms,
            finished_at_ms: inner.finished_at_ms,
            elapsed_ms: now.duration_since(self.created_at).as_millis() as u64,
            stage_timings: inner.timings_at(now),
            result: inner.result.clone(),
            error: inner.error.clone(),
            progress: inner.progress.clone(),
        }
    }
}

fn progress_event(
    run_id: &str,
    phase: ExecutionPhase,
    status: ProofProgressStatus,
    message: String,
    ctx: &ExecutionStepContext,
) -> RunActionProgress {
    RunActionProgress {
        run_id: run_id.to_string(),
        phase,
        status,
        message,
        old_root: ctx.old_root.as_ref().map(payload::encode_hash_hex),
        output_files: (!ctx.output_files.is_empty()).then(|| ctx.output_files.clone()),
        output_status: ctx.output_status,
        input_files: None,
        at_ms: now_ms(),
        stage: ctx.stage,
        elapsed_ms: 0,
    }
}

#[derive(Clone, Default)]
pub struct RunRegistry {
    pub entries: Registry<RunEntry>,
}

impl RunRegistry {
    pub fn get(&self, run_id: &str) -> Option<Arc<RunEntry>> {
        self.entries.get(run_id)
    }

    /// Every retained run, oldest first: in-flight runs plus terminal runs
    /// not yet reaped. `active_only` keeps just the in-flight ones.
    pub fn list(&self, active_only: bool) -> Vec<RunState> {
        self.entries
            .entries(active_only)
            .iter()
            .map(|entry| entry.snapshot())
            .collect()
    }
}

/// `ExecutionReporter` that fans every step out to both the global `/events`
/// broadcast (firehose subscribers) and the run's registry entry (which backs
/// `GET /runs/{id}` and its SSE stream).
#[derive(Clone)]
pub struct RunReporter {
    events: EventHub,
    entry: Arc<RunEntry>,
}

impl RunReporter {
    fn new(events: EventHub, entry: Arc<RunEntry>) -> Self {
        Self { events, entry }
    }

    fn run_id(&self) -> &str {
        &self.entry.run_id
    }

    fn emit(&self, progress: RunActionProgress) {
        let progress = self.entry.push_progress(progress);
        self.events.publish(Event::RunActionProgress(progress));
    }

    #[cfg(test)]
    fn commit_step(&self, message: &str, ctx: &ExecutionStepContext) {
        self.on_step(ExecutionPhase::Commit, message, ctx);
    }

    fn transaction_progress(
        &self,
        record: &TransactionRecord,
        status: ProofProgressStatus,
        message: &str,
        stage: Option<ExecutionStage>,
    ) {
        self.emit(RunActionProgress {
            run_id: self.run_id().to_string(),
            phase: ExecutionPhase::Commit,
            status,
            message: message.to_string(),
            old_root: Some(record.grounding_root.clone()),
            output_files: Some(record.output_files.clone()),
            output_status: Some(match record.state {
                TransactionState::Recorded | TransactionState::Expired => ObjectStatus::Unknown,
                TransactionState::Submitted => ObjectStatus::Pending,
                TransactionState::Landed => ObjectStatus::Live,
            }),
            input_files: Some(record.input_files.clone()),
            at_ms: now_ms(),
            stage,
            elapsed_ms: 0,
        });
    }

    fn finish_landed(&self, record: &TransactionRecord) {
        tracing::info!(run_id = %self.run_id(), tx_final = %record.tx_final, "Transaction landed");
        self.transaction_progress(
            record,
            ProofProgressStatus::Done,
            "Transaction landed",
            None,
        );
        self.entry.succeed(RunActionResult {
            run_id: self.run_id().to_string(),
            old_root: record.grounding_root.clone(),
            output_files: record.output_files.clone(),
            input_files: record.input_files.clone(),
        });
    }

    fn finish_failure(&self, message: String) {
        tracing::error!(run_id = %self.run_id(), error = %message, "Action failed");
        let event = self.entry.fail(message);
        self.events.publish(Event::RunActionProgress(event));
    }
}

impl ExecutionReporter for RunReporter {
    fn on_step(&self, phase: ExecutionPhase, message: &str, ctx: &ExecutionStepContext) {
        tracing::info!(run_id = %self.run_id(), ?phase, "{message}");
        self.emit(progress_event(
            self.run_id(),
            phase,
            ProofProgressStatus::Running,
            message.to_string(),
            ctx,
        ));
    }

    fn on_done(&self, phase: ExecutionPhase) {
        if phase == ExecutionPhase::GenerateProof {
            self.emit(progress_event(
                self.run_id(),
                phase,
                ProofProgressStatus::Done,
                "Proof generation complete".to_string(),
                &ExecutionStepContext::default(),
            ));
        }
    }
}

/// Start background execution and return a run handle.
pub fn spawn_run(services: &Services, input: ActionInput) -> RunAccepted {
    let input = crate::proof_jobs::trimmed(input);
    let services_for_worker = services.clone();
    let driver = services.driver.clone();
    let exec_input = crate::proof_jobs::execute_input(&input);
    spawn_worker(
        &services.runs,
        services.events.clone(),
        input.action,
        input.input_object_paths,
        move |reporter| {
            let record = crate::proof_jobs::prove_and_record(
                &services_for_worker,
                exec_input,
                reporter.run_id(),
                reporter,
            )?;
            submit_record(&services_for_worker, record, reporter)
        },
        move |tx_final| driver.get_transaction(&payload::decode_hash_hex(tx_final)?),
    )
}

/// Keep following the transaction after a submission error if its record is readable.
fn submit_record(
    services: &Services,
    record: TransactionRecord,
    reporter: &RunReporter,
) -> anyhow::Result<TransactionRecord> {
    reporter.transaction_progress(
        &record,
        ProofProgressStatus::Running,
        "Transaction recorded",
        Some(ExecutionStage::WritingFiles),
    );
    reporter.transaction_progress(
        &record,
        ProofProgressStatus::Running,
        "Submitting proof to relayer",
        Some(ExecutionStage::Submitting),
    );
    let tx_final = payload::decode_hash_hex(&record.tx_final)?;
    let record = match services.submit_transaction(&tx_final) {
        Ok(submitted) => {
            let message = match (submitted.state, submitted.relayer_job_id.as_deref()) {
                (TransactionState::Submitted, Some(job_id)) => {
                    format!("Relayer accepted job {job_id}")
                }
                _ => format!("Transaction {}", submitted.state),
            };
            reporter.transaction_progress(
                &submitted,
                ProofProgressStatus::Running,
                &message,
                Some(ExecutionStage::AwaitingSynchronization),
            );
            submitted
        }
        Err(err) => {
            let current = services.driver.get_transaction(&tx_final)?;
            reporter.transaction_progress(
                &current,
                ProofProgressStatus::Running,
                &format!(
                    "Relayer submission failed: {err:#}; transaction is {}",
                    current.state
                ),
                Some(ExecutionStage::Submitting),
            );
            services.announce_transaction(&current);
            current
        }
    };
    if record.is_open() {
        reporter.transaction_progress(
            &record,
            ProofProgressStatus::Running,
            "Waiting for the transaction to land",
            Some(ExecutionStage::AwaitingSynchronization),
        );
    }
    Ok(record)
}

/// Supervise worker errors and panics, then follow the transaction until settlement.
fn spawn_worker<S, R>(
    registry: &RunRegistry,
    events: EventHub,
    action: QualifiedName,
    input_object_paths: Vec<String>,
    stage: S,
    read_record: R,
) -> RunAccepted
where
    S: FnOnce(&RunReporter) -> anyhow::Result<TransactionRecord> + Send + 'static,
    R: Fn(&str) -> anyhow::Result<TransactionRecord> + Send + Sync + 'static,
{
    let run_id = uuid::Uuid::new_v4().to_string();
    let entry = registry.entries.start(run_id.clone(), |sequence| {
        RunEntry::new(run_id.clone(), action, input_object_paths, sequence)
    });
    let created_at_ms = entry.created_at_ms;
    let reporter = RunReporter::new(events.clone(), entry);

    tokio::spawn(async move {
        let worker = reporter.clone();
        let staged = tokio::task::spawn_blocking(move || stage(&worker)).await;
        let record = match staged {
            Ok(Ok(record)) => record,
            Ok(Err(err)) => return reporter.finish_failure(format!("{err:#}")),
            Err(join_err) => {
                return reporter.finish_failure(format!("run worker panicked: {join_err}"));
            }
        };
        let tx_final = record.tx_final.clone();
        let settled = crate::routes::wait::follow(
            &events,
            None,
            || {
                if record.state.is_terminal() {
                    Ok(record.clone())
                } else {
                    read_record(&tx_final)
                }
            },
            |record| record.state.is_terminal(),
            |event| transaction_event(&tx_final, event),
        )
        .await;
        match settled {
            Ok(settled) => match settled.state {
                TransactionState::Landed => reporter.finish_landed(&settled),
                TransactionState::Expired => reporter.finish_failure(format!(
                    "transaction {} expired without landing: {}",
                    settled.tx_final,
                    settled.reason.as_deref().unwrap_or("grounding aged out")
                )),
                TransactionState::Recorded | TransactionState::Submitted => {
                    unreachable!("follow returns only terminal records")
                }
            },
            Err(err) => {
                reporter.finish_failure(format!("lost track of transaction {tx_final}: {err:#}"))
            }
        }
    });

    RunAccepted {
        run_id,
        status: RunStatus::Queued,
        created_at_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

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

    fn start(registry: &RunRegistry, run_id: &str) -> Arc<RunEntry> {
        registry.entries.start(run_id.to_string(), |sequence| {
            RunEntry::new(run_id.to_string(), action("A"), Vec::new(), sequence)
        })
    }

    fn step(
        phase: ExecutionPhase,
        status: ProofProgressStatus,
        message: &str,
    ) -> RunActionProgress {
        progress_event(
            "r1",
            phase,
            status,
            message.to_string(),
            &ExecutionStepContext::default(),
        )
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
        let mut record = TransactionRecord::recorded(
            tx_final.to_string(),
            action("A"),
            "run".to_string(),
            vec!["in.dobj".to_string()],
            vec!["out.dobj".to_string()],
            "0xroot".to_string(),
            1,
            vec![],
            vec![],
            0,
        );
        record.state = state;
        record.reason = (state == TransactionState::Expired).then(|| "aged out".to_string());
        record
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

    #[test]
    fn stage_intervals_separate_mining_and_resumed_building() {
        let entry = RunEntry::new("r1".into(), action("A"), vec![], 0);
        let mut inner = entry.inner.lock().unwrap();
        let start = Instant::now();
        inner.begin_stage(ExecutionStage::Building, start, 100);
        inner.begin_stage(
            ExecutionStage::Mining,
            start + Duration::from_millis(20),
            120,
        );
        inner.begin_stage(
            ExecutionStage::Mining,
            start + Duration::from_millis(30),
            130,
        );
        let active = inner.timings_at(start + Duration::from_millis(60));
        assert_eq!(active.len(), 2);
        assert_eq!(active[0].elapsed_ms, 20);
        assert_eq!(active[1].elapsed_ms, 40);
        assert_eq!(active[1].status, ProofProgressStatus::Running);
        inner.begin_stage(
            ExecutionStage::Building,
            start + Duration::from_millis(70),
            170,
        );
        inner.finish_stage(
            ProofProgressStatus::Done,
            start + Duration::from_millis(100),
        );
        let finished = inner.timings_at(start + Duration::from_secs(100));
        assert_eq!(
            finished
                .iter()
                .map(|timing| timing.elapsed_ms)
                .collect::<Vec<_>>(),
            [20, 50, 30]
        );
        assert!(
            finished
                .iter()
                .all(|timing| timing.status == ProofProgressStatus::Done)
        );
        assert_eq!(
            finished
                .iter()
                .map(|timing| timing.stage)
                .collect::<Vec<_>>(),
            [
                ExecutionStage::Building,
                ExecutionStage::Mining,
                ExecutionStage::Building
            ]
        );
    }

    #[test]
    fn stage_progress_is_shared_by_broadcast_snapshots_and_replay() {
        let entry = Arc::new(RunEntry::new("r1".into(), action("A"), vec![], 0));
        let events = EventHub::new();
        let mut receiver = events.subscribe();
        let reporter = RunReporter::new(events, entry.clone());
        reporter.on_step(
            ExecutionPhase::GenerateProof,
            ExecutionStage::Proving.message(),
            &ExecutionStepContext {
                stage: Some(ExecutionStage::Proving),
                ..Default::default()
            },
        );
        let Event::RunActionProgress(broadcast) = receiver.try_recv().unwrap().event else {
            panic!("expected run progress");
        };
        let snapshot = entry.snapshot();
        assert_eq!(snapshot.progress[0], broadcast);
        assert_eq!(snapshot.progress[0], entry.events_from(0).0[0].1);
        assert_eq!(snapshot.stage_timings[0].stage, ExecutionStage::Proving);
        assert_eq!(
            snapshot.stage_timings[0].status,
            ProofProgressStatus::Running
        );
        assert!(snapshot.elapsed_ms >= broadcast.elapsed_ms);
        reporter.finish_failure("prover failed".into());
        let failed = entry.snapshot();
        assert_eq!(
            failed.progress.last().unwrap().stage,
            Some(ExecutionStage::Proving)
        );
        assert_eq!(failed.stage_timings[0].status, ProofProgressStatus::Failed);
        assert_eq!(entry.snapshot(), failed);
        assert!(entry.inner.lock().unwrap().stage_started.is_none());
    }

    #[test]
    fn transaction_progress_preserves_the_record_root_and_state() {
        let registry = RunRegistry::default();
        let entry = start(&registry, "run");
        let reporter = RunReporter::new(EventHub::new(), entry.clone());
        for (state, expected) in [
            (TransactionState::Recorded, ObjectStatus::Unknown),
            (TransactionState::Submitted, ObjectStatus::Pending),
            (TransactionState::Landed, ObjectStatus::Live),
            (TransactionState::Expired, ObjectStatus::Unknown),
        ] {
            let record = record("tx-1", state);
            reporter.transaction_progress(
                &record,
                ProofProgressStatus::Running,
                "Transaction updated",
                None,
            );
            let snapshot = entry.snapshot();
            let progress = snapshot.progress.last().unwrap();
            assert_eq!(
                progress.old_root.as_deref(),
                Some(record.grounding_root.as_str())
            );
            assert_eq!(progress.output_status, Some(expected));
            assert_eq!(progress.output_files.as_ref(), Some(&record.output_files));
            assert_eq!(progress.input_files.as_ref(), Some(&record.input_files));
        }
    }

    #[tokio::test]
    async fn runs_prove_concurrently_and_follow_their_own_transaction() {
        let registry = RunRegistry::default();
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
        );
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
        );
        wait_status(&registry, &second, RunStatus::Committing).await;
        assert_eq!(
            registry.get(&first.run_id).unwrap().status(),
            RunStatus::GenerateProof
        );

        release.send(()).unwrap();
        wait_status(&registry, &first, RunStatus::Committing).await;
        events.publish(Event::Transaction(record("tx-2", TransactionState::Landed)));
        assert_eq!(
            wait_terminal(&registry, &second).await.status,
            RunStatus::Succeeded
        );
        assert_eq!(
            registry.get(&first.run_id).unwrap().status(),
            RunStatus::Committing
        );

        events.publish(Event::Transaction(record("tx-1", TransactionState::Landed)));
        let landed = wait_terminal(&registry, &first).await;
        assert_eq!(landed.status, RunStatus::Succeeded);
        let result = landed.result.unwrap();
        assert_eq!(result.run_id, first.run_id);
        assert_eq!(result.output_files, vec!["out.dobj".to_string()]);
        assert_eq!(result.input_files, vec!["in.dobj".to_string()]);
        assert_eq!(result.old_root, "0xroot");
        let last = landed.progress.last().unwrap();
        assert_eq!(last.status, ProofProgressStatus::Done);
        assert_eq!(last.output_status, Some(ObjectStatus::Live));
    }

    #[tokio::test]
    async fn a_run_fails_when_its_transaction_expires_or_disappears() {
        let registry = RunRegistry::default();
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
        );
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
        );
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
        );
        assert_eq!(
            wait_terminal(&registry, &landed).await.status,
            RunStatus::Succeeded
        );
    }

    #[tokio::test]
    async fn worker_errors_and_panics_fail_the_run() {
        let registry = RunRegistry::default();
        let events = EventHub::new();
        let refused = spawn_worker(
            &registry,
            events.clone(),
            action("refused"),
            Vec::new(),
            |_| Err(anyhow::anyhow!("input is held by another run")),
            still_open,
        );
        assert_eq!(
            wait_terminal(&registry, &refused).await.error.as_deref(),
            Some("input is held by another run")
        );
        let panicked = spawn_worker(
            &registry,
            events.clone(),
            action("panic"),
            Vec::new(),
            |reporter| {
                reporter.on_step(
                    ExecutionPhase::GenerateProof,
                    ExecutionStage::Proving.message(),
                    &ExecutionStepContext {
                        stage: Some(ExecutionStage::Proving),
                        ..Default::default()
                    },
                );
                panic!("worker panic");
            },
            still_open,
        );
        let state = wait_terminal(&registry, &panicked).await;
        assert_eq!(state.status, RunStatus::Failed);
        assert_eq!(state.stage_timings[0].stage, ExecutionStage::Proving);
        assert_eq!(state.stage_timings[0].status, ProofProgressStatus::Failed);
        assert_eq!(registry.get(&panicked.run_id).unwrap().snapshot(), state);
        assert!(state.error.unwrap().contains("worker panic"));
    }

    #[test]
    fn run_lifecycle_preserves_inputs_timestamps_and_replay() {
        let inputs = vec!["a.dobj".to_string(), "b.dobj".to_string()];
        let entry = RunEntry::new("r1".to_string(), action("A"), inputs.clone(), 0);
        let queued = entry.snapshot();
        assert_eq!(queued.status, RunStatus::Queued);
        assert_eq!(queued.input_object_paths, inputs);
        assert!(queued.created_at_ms > 0);
        assert_eq!(queued.started_at_ms, None);
        assert_eq!(queued.finished_at_ms, None);

        let mut first = step(
            ExecutionPhase::GenerateProof,
            ProofProgressStatus::Running,
            "gen",
        );
        first.stage = Some(ExecutionStage::Proving);
        let first_at_ms = first.at_ms;
        entry.push_progress(first);
        assert_eq!(entry.status(), RunStatus::GenerateProof);
        let mut committing = step(
            ExecutionPhase::Commit,
            ProofProgressStatus::Running,
            "commit",
        );
        committing.at_ms = first_at_ms + 1;
        committing.stage = Some(ExecutionStage::AwaitingSynchronization);
        entry.push_progress(committing);
        let running = entry.snapshot();
        assert_eq!(running.status, RunStatus::Committing);
        assert_eq!(running.started_at_ms, Some(first_at_ms));
        assert_eq!(running.finished_at_ms, None);

        let (all, terminal) = entry.events_from(0);
        assert_eq!(
            all.iter().map(|(index, _)| *index).collect::<Vec<_>>(),
            [0, 1]
        );
        assert!(!terminal);
        let (tail, _) = entry.events_from(1);
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].0, 1);
        assert_eq!(tail[0].1.message, "commit");

        entry.succeed(ok_result());
        let finished = entry.snapshot();
        assert_eq!(finished.status, RunStatus::Succeeded);
        assert_eq!(entry.snapshot(), finished);
        assert_eq!(finished.stage_timings.len(), 2);
        assert!(
            finished
                .stage_timings
                .iter()
                .all(|timing| timing.status == ProofProgressStatus::Done)
        );
        assert_eq!(finished.result, Some(ok_result()));
        assert!(finished.error.is_none());
        assert_eq!(finished.progress, running.progress);
        assert!(finished.finished_at_ms.unwrap() >= first_at_ms);
        assert!(entry.events_from(0).1);
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
        assert_eq!(snapshot.started_at_ms, Some(event.at_ms));
        assert_eq!(snapshot.finished_at_ms, Some(event.at_ms));
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
    fn reaper_keeps_active_runs_and_retains_terminal_runs_until_expired() {
        let registry = RunRegistry::default();
        start(&registry, "active");
        start(&registry, "succeeded").succeed(ok_result());
        start(&registry, "failed").fail("stopped".to_string());
        registry.entries.reap(crate::registry::RETENTION);
        for id in ["active", "succeeded", "failed"] {
            assert!(registry.get(id).is_some());
        }
        registry.entries.reap(Duration::ZERO);
        assert!(registry.get("active").is_some());
        assert!(registry.get("succeeded").is_none());
        assert!(registry.get("failed").is_none());
    }

    #[test]
    fn list_is_oldest_first_and_can_keep_only_active_runs() {
        let registry = RunRegistry::default();
        for run_id in ["first", "second", "third"] {
            start(&registry, run_id);
        }
        registry.get("second").unwrap().succeed(ok_result());

        let run_ids = |runs: Vec<RunState>| -> Vec<String> {
            runs.into_iter().map(|run| run.run_id).collect()
        };
        assert_eq!(run_ids(registry.list(false)), ["first", "second", "third"]);
        assert_eq!(run_ids(registry.list(true)), ["first", "third"]);
    }

    #[test]
    fn reporter_broadcasts_the_retained_progress_for_success_and_failure() {
        for failed in [false, true] {
            let events = EventHub::new();
            let mut rx = events.subscribe();
            let entry = Arc::new(RunEntry::new("r1".to_string(), action("A"), Vec::new(), 0));
            let reporter = RunReporter::new(events, entry.clone());
            reporter.on_step(ExecutionPhase::GenerateProof, "gen", &Default::default());
            reporter.on_done(ExecutionPhase::GenerateProof);
            reporter.on_step(ExecutionPhase::Commit, "commit", &Default::default());
            if failed {
                reporter.finish_failure("kaboom".to_string());
            } else {
                reporter.finish_landed(&record("tx-1", TransactionState::Landed));
            }
            let progress = entry.snapshot().progress;
            assert_eq!(
                progress
                    .iter()
                    .map(|event| event.status)
                    .collect::<Vec<_>>(),
                [
                    ProofProgressStatus::Running,
                    ProofProgressStatus::Done,
                    ProofProgressStatus::Running,
                    if failed {
                        ProofProgressStatus::Failed
                    } else {
                        ProofProgressStatus::Done
                    },
                ]
            );
            for retained in progress {
                let Event::RunActionProgress(broadcast) = rx.try_recv().unwrap().event else {
                    panic!("expected run progress");
                };
                assert_eq!(broadcast, retained);
            }
            assert!(rx.try_recv().is_err());
        }
    }
}
