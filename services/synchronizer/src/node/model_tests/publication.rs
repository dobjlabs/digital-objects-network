use super::*;
use crate::model_hooks::{next, Gate, Stage};
use tokio::sync::mpsc;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(tag = "tag")]
enum SlotKind {
    Execution,
    WithoutExecution,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct ReadSnapshot {
    progress: i64,
    state_slot: i64,
    created: Vec<u32>,
}

impl ReadSnapshot {
    fn empty() -> Self {
        Self {
            progress: -1,
            state_slot: -1,
            created: vec![],
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(tag = "tag")]
enum PublicationPhase {
    Idle,
    Flushed,
    SlotInserted,
    IndexInserted,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct PublicationState {
    phase: PublicationPhase,
    committed: Vec<SlotKind>,
    durable: bool,
    published: ReadSnapshot,
    result: ReadSnapshot,
}

struct PublicationDriver {
    base: ModelDriver,
    phase: PublicationPhase,
    baseline_flushes: u64,
    gate: Option<Arc<Gate>>,
    events: Option<mpsc::UnboundedReceiver<Stage>>,
    writer: Option<JoinHandle<Result<()>>>,
    read_gate: Option<Arc<Gate>>,
    reader: Option<JoinHandle<Result<ReadSnapshot>>>,
    result: ReadSnapshot,
}

impl PublicationDriver {
    fn new() -> Self {
        Self {
            base: ModelDriver::new().expect("publication driver setup failed"),
            phase: PublicationPhase::Idle,
            baseline_flushes: 0,
            gate: None,
            events: None,
            writer: None,
            read_gate: None,
            reader: None,
            result: ReadSnapshot::empty(),
        }
    }

    fn committed(&self) -> Result<Vec<SlotKind>> {
        let states: Vec<bool> = self.base.runtime.block_on(
            sqlx::query_scalar("SELECT el_block_number IS NOT NULL FROM slots ORDER BY slot")
                .fetch_all(&self.base.pool),
        )?;
        Ok(states
            .into_iter()
            .map(|derived| {
                if derived {
                    SlotKind::Execution
                } else {
                    SlotKind::WithoutExecution
                }
            })
            .collect())
    }

    fn snapshot(&self) -> Result<ReadSnapshot> {
        let snapshot = self.base.runtime.block_on(self.base.snapshot())?;
        Ok(match snapshot.states.last() {
            Some(state) => ReadSnapshot {
                progress: i64::from(snapshot.committed.last().context("missing slot")?.slot),
                state_slot: i64::from(state.slot),
                created: state.created.clone(),
            },
            None => ReadSnapshot::empty(),
        })
    }

    async fn discard_tasks(&mut self) {
        if let Some(task) = self.writer.take() {
            task.abort();
            let _ = task.await;
        }
        if let Some(task) = self.reader.take() {
            task.abort();
            let _ = task.await;
        }
        if let Some(node) = &self.base.node {
            *node.state_log.publish_gate.lock().unwrap() = None;
            *node.state_log.read_gate.lock().unwrap() = None;
        }
        self.gate = None;
        self.events = None;
        self.read_gate = None;
        self.phase = PublicationPhase::Idle;

        self.result = ReadSnapshot::empty();
    }

    async fn start(&mut self, kind: SlotKind) -> Result<()> {
        self.base.fetch_slots(1, false).await?;
        let FetchedSlot::Present { mut context, .. } =
            self.base.fetched.pop_front().context("missing candidate")?
        else {
            anyhow::bail!("publication fixture must produce a block");
        };
        let node = Arc::clone(self.base.node()?);
        if kind == SlotKind::WithoutExecution {
            context.payload = None;
            context.blob_payloads.clear();
        }
        let genesis = node.state_log.head_state().await?.is_none();
        if genesis {
            node.validate_genesis_slot(1).await?;
        }
        let candidate = node.derive_from_context(*context).await?;
        if genesis {
            self.base.anchor_root = Some(
                candidate
                    .derived
                    .as_ref()
                    .context("genesis needs a state")?
                    .state
                    .metadata
                    .state_root,
            );
        }

        self.baseline_flushes = self.base.containers.as_ref().unwrap().flush_count();
        let (gate, events) = Gate::new();
        *node.state_log.publish_gate.lock().unwrap() = Some(Arc::clone(&gate));
        self.writer = Some(tokio::spawn(
            async move { node.commit_slot(&candidate).await },
        ));
        self.gate = Some(gate);
        self.events = Some(events);
        self.observe().await
    }

    async fn advance(&mut self) -> Result<()> {
        self.gate.as_ref().context("writer not started")?.proceed();
        if self.phase == PublicationPhase::IndexInserted {
            self.writer.take().context("missing writer")?.await??;
            *self.base.node()?.state_log.publish_gate.lock().unwrap() = None;
            self.phase = PublicationPhase::Idle;
            self.gate = None;
            self.events = None;
            Ok(())
        } else {
            self.observe().await
        }
    }

    async fn observe(&mut self) -> Result<()> {
        let stage = next(self.events.as_mut().context("missing event receiver")?).await?;
        self.phase = match stage {
            Stage::Flushed => PublicationPhase::Flushed,
            Stage::SlotInserted => PublicationPhase::SlotInserted,
            Stage::IndexInserted => PublicationPhase::IndexInserted,
            _ => anyhow::bail!("unexpected publication checkpoint {stage:?}"),
        };
        Ok(())
    }

    async fn begin_read(&mut self) -> Result<()> {
        let node = Arc::clone(self.base.node()?);
        let containers = self.base.containers.as_ref().unwrap().clone();
        let (gate, mut events) = Gate::new();
        *node.state_log.read_gate.lock().unwrap() = Some(Arc::clone(&gate));
        self.reader = Some(tokio::spawn(async move {
            let objects: Vec<_> = (11..=13).map(object_hash).collect();
            let (snapshot, indices) = node
                .state_log
                .snapshot_with_created_indices(&objects)
                .await?;
            let Some(snapshot) = snapshot else {
                return Ok(ReadSnapshot::empty());
            };
            let present =
                containers.created_exists_for(&snapshot.state.roots, &objects, &indices)?;
            Ok(ReadSnapshot {
                progress: i64::from(snapshot.last_processed_slot) - 1,
                state_slot: i64::from(snapshot.state.metadata.block.number),
                created: (11..=13)
                    .zip(present)
                    .filter_map(|(object, present)| present.then_some(object))
                    .collect(),
            })
        }));
        ensure!(
            next(&mut events).await? == Stage::ReadSnapshot,
            "reader did not capture a snapshot"
        );
        self.read_gate = Some(gate);

        Ok(())
    }
}

impl quint_connect::State<PublicationDriver> for PublicationState {
    fn from_driver(driver: &PublicationDriver) -> Result<Self> {
        Ok(Self {
            published: driver.snapshot()?,
            phase: driver.phase,
            committed: driver.committed()?,
            durable: driver
                .base
                .containers
                .as_ref()
                .context("missing app store")?
                .flush_count()
                > driver.baseline_flushes,
            result: driver.result.clone(),
        })
    }
}

impl Driver for PublicationDriver {
    type State = PublicationState;

    fn config() -> Config {
        Config {
            state: &["state"],
            nondet: &["command"],
        }
    }

    fn step(&mut self, step: &Step) -> Result<()> {
        let runtime = Arc::clone(&self.base.runtime);
        switch!(step {
            Init(genesis: bool) => {
                runtime.block_on(self.discard_tasks());
                let layout = Layout { fork_point: 2, ..Layout::default() };
                if genesis {
                    runtime.block_on(self.base.initialize(layout))?;
                } else {
                    runtime.block_on(self.base.reset(layout))?;
                }
                self.baseline_flushes = self.base.containers.as_ref().unwrap().flush_count();
            },
            Start(kind: SlotKind) => runtime.block_on(self.start(kind))?,
            Advance => runtime.block_on(self.advance())?,
            Crash => {
                runtime.block_on(self.discard_tasks());
                runtime.block_on(self.base.restart())?;
                self.baseline_flushes = self.base.containers.as_ref().unwrap().flush_count();
            },
            Rollback => {
                let node = self.base.node()?;
                runtime.block_on(async {
                    node.state_log.rollback_to_slot(node.state_log.last_processed_slot().await?.context("missing genesis")? - 1)
                        .await
                })?;
            },
            BeginRead => {

                runtime.block_on(self.begin_read())?;
            },
            FinishRead => {
                self.read_gate.as_ref().context("missing reader gate")?.proceed();
                self.result = runtime.block_on(self.reader.take().context("missing reader")?)??;
                *self.base.node()?.state_log.read_gate.lock().unwrap() = None;

            },
            CloseRead => {

                self.result = ReadSnapshot::empty();
            },
        })
    }
}

impl Drop for PublicationDriver {
    fn drop(&mut self) {
        let runtime = Arc::clone(&self.base.runtime);
        runtime.block_on(self.discard_tasks());
    }
}

#[quint_run(spec = "model/publication.qnt", max_samples = 60, max_steps = 35)]
#[ignore = "requires Postgres and quint on PATH"]
fn quint_model_publication_generated() -> impl Driver {
    PublicationDriver::new()
}

macro_rules! scenarios {
    ($($name:ident => $scenario:literal),* $(,)?) => {
        $(
            #[quint_test(spec = "model/publication.qnt", test = $scenario, max_samples = 1)]
            #[ignore = "requires Postgres and quint on PATH"]
            fn $name() -> impl Driver {
                PublicationDriver::new()
            }
        )*
    };
}

scenarios! {
    quint_model_publication_read_rollback => "rollbackDuringReadTest",
    quint_model_publication_crash_commit => "crashAfterCommitTest",
    quint_model_publication_abort_transaction => "crashBeforeCommitTest",
    quint_model_publication_read_publish => "publishDuringReadTest",
    quint_model_publication_genesis_abort => "genesisCrashBeforeCommitTest",
    quint_model_publication_genesis_commit => "genesisCrashAfterCommitTest",
    quint_model_publication_genesis_read => "genesisDuringReadTest",
    quint_model_publication_progress => "progressWithoutStateTest",
}
