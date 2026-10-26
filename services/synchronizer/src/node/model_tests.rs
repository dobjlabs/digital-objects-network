use std::{
    collections::{BTreeSet, HashMap, VecDeque},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, RwLock,
    },
    time::Duration,
};

use alloy::primitives::{Address, B256};
use anyhow::{anyhow, ensure, Context, Result};
use axum::{
    extract::Path, extract::State, http::StatusCode, routing::get, routing::post, Json, Router,
};
use eth_clients::common::AvailabilityPolling;
use payload::proof::MockBlobParser;
use pod2::middleware::{Hash, Value};
use pod2utils::b256_to_hash;
use quint_connect::{quint_run, quint_test, switch, Config, Driver, Step};
use serde::Deserialize;
use serde_json::{json, Value as JsonValue};
use sqlx::{postgres::PgPoolOptions, PgPool, Row};
use tempfile::TempDir;
use tokio::{runtime::Runtime, task::JoinHandle};

use super::Node;
use crate::{
    catchup::{fetch_batch, FetchedSlot},
    config::AppConfig,
    container_db::{db_bytes_to_hash, ContainerDb},
    state_log_db::{decode_state_row, StateLogDb},
    state_machine::StateMachine,
    sync_loop::{process_fetched_slot, SlotAction},
};

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct ModelBlock {
    slot: u32,
    root: u32,
    parent: u32,
    has_payload: bool,
    creates: Vec<u32>,
    spends: BTreeSet<u32>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct Layout {
    fork_point: u32,
    first_gaps: BTreeSet<u32>,
    second_gaps: BTreeSet<u32>,
}

impl Layout {
    fn root(&self, fork: u32, slot: u32) -> u32 {
        if slot == 0 {
            return 1;
        }
        let gaps = if fork == 1 {
            &self.first_gaps
        } else {
            &self.second_gaps
        };
        if gaps.contains(&slot) {
            return 0;
        }
        (if slot < self.fork_point {
            10
        } else {
            fork * 10
        }) + slot
    }
}

impl ModelBlock {
    fn at(fork: u32, layout: &Layout, slot: u32) -> Self {
        let root = layout.root(fork, slot);
        let parent_slot = (0..slot)
            .rev()
            .find(|previous| layout.root(fork, *previous) != 0)
            .unwrap_or(0);
        let produced = slot != 0 && root != 0;
        Self {
            slot,
            root,
            parent: if produced {
                layout.root(fork, parent_slot)
            } else {
                0
            },
            has_payload: slot == 0 || (produced && slot != 5),
            creates: if produced && slot <= 4 {
                vec![root]
            } else {
                Vec::new()
            },
            spends: if produced && slot == 4 && layout.root(fork, 2) != 0 {
                BTreeSet::from([layout.root(fork, 2)])
            } else {
                BTreeSet::new()
            },
        }
    }

    fn from_header(
        header: &eth_clients::beacon::types::BlockHeader,
        layout: &Layout,
    ) -> Result<Self> {
        let root = hash_id(header.root)?;
        let mut block = Self::at(root / 10, layout, header.slot - 1);
        block.root = root;
        block.parent = hash_id(header.parent_root)?;
        Ok(block)
    }
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct ModelDonState {
    slot: u32,
    created: Vec<u32>,
    nullifiers: BTreeSet<u32>,
    history: Vec<u32>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct ModelIndexEntry {
    commitment: u32,
    index: usize,
    slot: u32,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct ModelState {
    committed: Vec<ModelBlock>,
    states: Vec<ModelDonState>,
    created_index: Vec<ModelIndexEntry>,
    fetched: Vec<ModelBlock>,
}

impl quint_connect::State<ModelDriver> for ModelState {
    fn from_driver(driver: &ModelDriver) -> Result<Self> {
        driver.runtime.block_on(driver.snapshot())
    }
}

#[derive(Default)]
struct Chain {
    fork: u32,
    layout: Layout,
    header_forks: HashMap<u32, u32>,
}

fn block_hash(id: u32) -> B256 {
    B256::repeat_byte(u8::try_from(id).expect("fixture identity fits in one byte"))
}

fn object_hash(id: u32) -> Hash {
    b256_to_hash(block_hash(id))
}

fn hash_id(hash: B256) -> Result<u32> {
    let id = u32::from(hash[0]);
    ensure!(hash == block_hash(id), "unrecognized fixture block hash");
    Ok(id)
}

fn object_id(value: Value) -> Result<u32> {
    (1..=26)
        .find(|id| value == Value::from(object_hash(*id)))
        .ok_or_else(|| anyhow!("unrecognized persisted object commitment"))
}

async fn beacon_header(
    State(chain): State<Arc<RwLock<Chain>>>,
    Path(slot): Path<String>,
) -> Result<Json<JsonValue>, StatusCode> {
    let slot: u32 = if slot == "finalized" {
        1
    } else {
        slot.parse().map_err(|_| StatusCode::BAD_REQUEST)?
    };
    let slot = slot.checked_sub(1).ok_or(StatusCode::BAD_REQUEST)?;
    let chain = chain.read().unwrap();
    let fork = chain.header_forks.get(&slot).copied().unwrap_or(chain.fork);
    let block = ModelBlock::at(fork, &chain.layout, slot);
    if block.root == 0 {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(Json(json!({ "data": {
        "root": block_hash(block.root),
        "header": { "message": {
            "slot": (slot + 1).to_string(), "parent_root": block_hash(block.parent),
        }},
    }})))
}

async fn beacon_block(
    State(chain): State<Arc<RwLock<Chain>>>,
    Path(root): Path<B256>,
) -> Result<Json<JsonValue>, StatusCode> {
    let root = hash_id(root).map_err(|_| StatusCode::BAD_REQUEST)?;
    let slot = if root == 1 { 0 } else { root % 10 };
    let block = ModelBlock::at(root / 10, &chain.read().unwrap().layout, slot);
    let body = if block.has_payload || slot == 0 {
        json!({ "execution_payload": { "block_hash": block_hash(root) }, "blob_kzg_commitments": [] })
    } else {
        json!({ "signed_execution_payload_bid": { "message": {
            "block_hash": block_hash(root), "parent_block_hash": B256::ZERO,
            "blob_kzg_commitments": [],
        }}})
    };
    Ok(Json(json!({ "data": { "message": {
        "slot": (slot + 1).to_string(), "parent_root": block_hash(block.parent), "body": body,
    }}})))
}

async fn execution_rpc(Json(request): Json<JsonValue>) -> Result<Json<JsonValue>, StatusCode> {
    if request["method"] != "eth_getBlockByHash" {
        return Err(StatusCode::BAD_REQUEST);
    }
    let hash: B256 = serde_json::from_value(request["params"][0].clone())
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let root = hash_id(hash).map_err(|_| StatusCode::BAD_REQUEST)?;
    let mut block = alloy::rpc::types::Block::<alloy::rpc::types::Transaction>::default();
    block.header.hash = hash;
    block.header.inner.number = if root == 1 { 0 } else { u64::from(root % 10) };
    block.header.inner.timestamp = u64::from(root) * 12;
    Ok(Json(
        json!({ "jsonrpc": "2.0", "id": request["id"], "result": block }),
    ))
}

struct ModelDriver {
    runtime: Arc<Runtime>,
    server: JoinHandle<()>,
    chain: Arc<RwLock<Chain>>,
    base_url: String,
    storage: TempDir,
    admin: PgPool,
    database_name: String,
    database_url: String,
    pool: PgPool,
    node: Option<Arc<Node>>,
    containers: Option<ContainerDb>,
    anchor_root: Option<Hash>,
    fetched: VecDeque<FetchedSlot>,
}

impl ModelDriver {
    fn new() -> Result<Self> {
        static DATABASE_COUNTER: AtomicU64 = AtomicU64::new(0);
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()?,
        );
        let chain = Arc::new(RwLock::new(Chain::default()));
        let database_name = format!(
            "quint_sync_{}_{}",
            std::process::id(),
            DATABASE_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let admin_url = std::env::var("TEST_PG_ADMIN_URL")
            .unwrap_or_else(|_| "postgres://postgres:postgres@127.0.0.1:5432/postgres".into());
        let mut url = reqwest::Url::parse(&admin_url)?;
        url.set_path(&format!("/{database_name}"));
        let database_url = url.to_string();
        let (admin, pool, server, base_url) = runtime.block_on(async {
            let admin = PgPoolOptions::new()
                .max_connections(1)
                .connect(&admin_url)
                .await?;
            sqlx::query(&format!("CREATE DATABASE {database_name}"))
                .execute(&admin)
                .await?;
            let pool = PgPoolOptions::new()
                .max_connections(2)
                .connect(&database_url)
                .await?;
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
            let base_url = format!("http://{}", listener.local_addr()?);
            let router = Router::new()
                .route("/eth/v1/beacon/headers/{slot}", get(beacon_header))
                .route("/eth/v2/beacon/blocks/{root}", get(beacon_block))
                .route("/rpc", post(execution_rpc))
                .with_state(Arc::clone(&chain));
            let server = tokio::spawn(async move {
                axum::serve(listener, router)
                    .await
                    .expect("fake chain API failed");
            });
            Ok::<_, anyhow::Error>((admin, pool, server, base_url))
        })?;
        Ok(Self {
            runtime,
            server,
            chain,
            base_url,
            storage: TempDir::new()?,
            admin,
            database_name,
            database_url,
            pool,
            node: None,
            containers: None,
            anchor_root: None,
            fetched: VecDeque::new(),
        })
    }

    fn node(&self) -> Result<&Arc<Node>> {
        self.node.as_ref().context("driver is not initialized")
    }

    async fn open_node(&mut self) -> Result<()> {
        let app_path = self.storage.path().join("rocksdb");
        let containers = ContainerDb::connect(app_path.to_str().context("invalid store path")?)?;
        let state_machine = Arc::new(StateMachine::new(
            containers.clone(),
            Arc::new(MockBlobParser),
        ));
        let state_log = Arc::new(StateLogDb::connect(&self.database_url, 1).await?);
        let cfg = AppConfig {
            container_db_path: app_path.to_string_lossy().into_owned(),
            state_log_db_url: self.database_url.clone(),
            http_bind: "127.0.0.1:0".parse()?,
            sync_delay: Duration::ZERO,
            availability_polling: AvailabilityPolling {
                retries: 0,
                delay: Duration::ZERO,
            },
            catchup_batch_size: 2,
            init_start_slot: 1,
            rpc_url: format!("{}/rpc", self.base_url),
            beacon_url: self.base_url.clone(),
            archiver_url: self.base_url.clone(),
            to_address: Address::ZERO,
        };
        self.node = Some(Arc::new(Node::new(cfg, state_machine, state_log).await?));
        self.containers = Some(containers);
        Ok(())
    }

    async fn reset(&mut self, config: Layout) -> Result<()> {
        self.anchor_root = None;
        self.fetched.clear();
        self.node.take();
        self.containers.take();
        let app_path = self.storage.path().join("rocksdb");
        if app_path.exists() {
            std::fs::remove_dir_all(app_path)?;
        }
        *self.chain.write().unwrap() = Chain {
            fork: 1,
            layout: config,
            header_forks: HashMap::new(),
        };
        self.open_node().await?;
        sqlx::query("TRUNCATE slots, created_index")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn initialize(&mut self, config: Layout) -> Result<()> {
        self.reset(config).await?;
        let node = self.node()?;
        node.validate_genesis_slot(1).await?;
        let genesis = fetch_batch(node, &[1])
            .await?
            .into_iter()
            .next()
            .context("missing genesis")?;
        ensure!(
            matches!(
                process_fetched_slot(node, genesis).await?,
                SlotAction::Applied
            ),
            "genesis processing rewound"
        );
        let state = node
            .state_log
            .head_state()
            .await?
            .context("genesis has no state")?;
        self.anchor_root = Some(state.metadata.state_root);
        Ok(())
    }

    async fn fetch_slots(&mut self, count: u32, change_fork: bool) -> Result<()> {
        ensure!(
            self.fetched.is_empty(),
            "fetch while another batch is pending"
        );
        let next = self
            .node()?
            .state_log
            .last_processed_slot()
            .await?
            .unwrap_or(0);
        {
            let mut chain = self.chain.write().unwrap();
            let initial = chain.fork;
            if change_fork {
                chain.fork = 3 - initial;
            }
            for slot in next..next + count {
                let fork = if slot == next { initial } else { chain.fork };
                chain.header_forks.insert(slot, fork);
            }
        }
        let slots: Vec<_> = (next..next + count).map(|slot| slot + 1).collect();
        let mut fetched = fetch_batch(self.node()?, &slots).await?;
        let layout = {
            let mut chain = self.chain.write().unwrap();
            chain.header_forks.clear();
            chain.layout.clone()
        };
        for slot in &mut fetched {
            if let FetchedSlot::Present {
                header, context, ..
            } = slot
            {
                let block = ModelBlock::from_header(header, &layout)?;
                if block.has_payload && (!block.creates.is_empty() || !block.spends.is_empty()) {
                    let hashes = |objects: Vec<u32>| {
                        objects
                            .into_iter()
                            .map(|id| format!("{:#}", object_hash(id)))
                            .collect::<Vec<_>>()
                    };
                    let blob = serde_json::to_vec(&json!({
                        "tx_final": format!("{:#}", object_hash(block.root)),
                        "created": hashes(block.creates),
                        "nullifiers": hashes(block.spends.into_iter().collect()),
                        "state_root": format!("{:#}", self.anchor_root.context("missing anchor")?),
                    }))?;
                    context.blob_payloads = vec![(0, blob)];
                }
            }
        }
        self.fetched = fetched.into();
        Ok(())
    }

    async fn process_next(&mut self) -> Result<()> {
        let fetched = self
            .fetched
            .pop_front()
            .context("no fetched slot to process")?;
        if let SlotAction::Rewound(_) = process_fetched_slot(self.node()?, fetched).await? {
            self.fetched.clear();
        }
        Ok(())
    }

    async fn restart(&mut self) -> Result<()> {
        self.fetched.clear();
        self.node.take();
        self.containers.take();
        self.open_node().await
    }

    async fn snapshot(&self) -> Result<ModelState> {
        let containers = self
            .containers
            .as_ref()
            .context("driver is not initialized")?;
        let rows = sqlx::query("SELECT * FROM slots ORDER BY slot")
            .fetch_all(&self.pool)
            .await?;
        let layout = self.chain.read().unwrap().layout.clone();
        let mut root_ids = HashMap::new();
        for row in &rows {
            if let Some(bytes) = row.get::<Option<Vec<u8>>, _>("don_state_root") {
                let root = row.get::<Vec<u8>, _>("cl_block_root");
                root_ids.insert(db_bytes_to_hash(&bytes)?, hash_id(B256::from_slice(&root))?);
            }
        }
        let mut committed = Vec::new();
        let mut states = Vec::new();
        for row in &rows {
            let slot = row.get::<i32, _>("slot") as u32 - 1;
            let root = row
                .get::<Option<Vec<u8>>, _>("cl_block_root")
                .map(|bytes| hash_id(B256::from_slice(&bytes)))
                .transpose()?
                .unwrap_or(0);
            let mut block = ModelBlock::at(root / 10, &layout, slot);
            block.root = root;
            block.has_payload = row.get::<Option<i32>, _>("el_block_number").is_some();
            if root == 0 {
                block.parent = 0;
                block.creates.clear();
                block.spends.clear();
            }
            let has_payload = block.has_payload;
            committed.push(block);
            if !has_payload {
                ensure!(
                    row.get::<Option<Vec<u8>>, _>("don_state_root").is_none(),
                    "slot without execution block stored a DON state"
                );
                continue;
            }
            let don_state = decode_state_row(row)?;
            ensure!(
                don_state.state_header().hash() == don_state.metadata.state_root,
                "published state header hash mismatch"
            );
            let created_array = containers.open_created(don_state.roots.created)?;
            let mut created = Vec::new();
            for index in 0..don_state.metadata.created_count as usize {
                created.push(object_id(
                    created_array.get(index)?.context("missing created leaf")?,
                )?);
            }
            ensure!(
                created_array.dump()?.len() == created.len(),
                "created count mismatch"
            );
            let nullifiers = containers
                .open_nullifiers(don_state.roots.nullifiers)?
                .dump()?
                .into_iter()
                .map(object_id)
                .collect::<Result<BTreeSet<_>>>()?;
            ensure!(
                nullifiers.len() as u64 == don_state.metadata.nullifier_count,
                "nullifier count mismatch"
            );
            let history_array =
                containers.open_next_state_history(don_state.roots.next_state_history)?;
            ensure!(
                history_array.dump()?.len() as u64 == don_state.metadata.state_root_count,
                "history count mismatch"
            );
            ensure!(
                history_array.get(0)? == self.anchor_root.map(Value::from),
                "missing retained anchor"
            );
            let mut history = Vec::new();
            for index in 0..don_state.metadata.state_root_count as usize {
                let value = history_array.get(index)?.context("missing history leaf")?;
                history.push(
                    *root_ids
                        .get(&Hash::from(value.raw()))
                        .context("history contains an uncommitted root")?,
                );
            }
            states.push(ModelDonState {
                slot,
                created,
                nullifiers,
                history,
            });
        }
        let index_rows = sqlx::query(
            "SELECT commitment, array_index, slot FROM created_index ORDER BY array_index",
        )
        .fetch_all(&self.pool)
        .await?;
        let created_index = index_rows
            .iter()
            .map(|row| {
                Ok(ModelIndexEntry {
                    commitment: object_id(Value::from(db_bytes_to_hash(
                        &row.get::<Vec<u8>, _>("commitment"),
                    )?))?,
                    index: row.get::<i64, _>("array_index") as usize,
                    slot: row.get::<i32, _>("slot") as u32 - 1,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let fetched = self
            .fetched
            .iter()
            .map(|slot| match slot {
                FetchedSlot::Missing { slot } => Ok(ModelBlock {
                    slot: *slot - 1,
                    root: 0,
                    parent: 0,
                    has_payload: false,
                    creates: vec![],
                    spends: BTreeSet::new(),
                }),
                FetchedSlot::Present { header, .. } => ModelBlock::from_header(header, &layout),
            })
            .collect::<Result<Vec<_>>>()?;
        let snapshot = self.node()?.state_log.head_snapshot().await?;
        if let Some(snapshot) = snapshot {
            ensure!(
                snapshot.last_processed_slot == committed.last().context("missing slot")?.slot + 1,
                "head progress disagrees with the slot log"
            );
            let state_slot = states.last().context("missing derived state")?.slot;
            ensure!(
                snapshot.state == decode_state_row(&rows[state_slot as usize])?,
                "head did not select the latest derived state"
            );
        } else {
            ensure!(
                committed.is_empty(),
                "published slots have no genesis state"
            );
        }
        Ok(ModelState {
            committed,
            states,
            created_index,
            fetched,
        })
    }
}

impl Driver for ModelDriver {
    type State = ModelState;

    fn config() -> Config {
        Config {
            state: &[],
            nondet: &["command"],
        }
    }

    fn step(&mut self, step: &Step) -> Result<()> {
        let runtime = Arc::clone(&self.runtime);
        switch!(step {
            Init(config: Layout) => runtime.block_on(self.initialize(config))?,
            SwitchFork => {
                let mut chain = self.chain.write().unwrap();
                chain.fork = 3 - chain.fork;
            },
            Fetch(count: u32, change_fork: bool) => runtime.block_on(self.fetch_slots(count, change_fork))?,
            Process => runtime.block_on(self.process_next())?,
            Restart => runtime.block_on(self.restart())?,
        })
    }
}

impl Drop for ModelDriver {
    fn drop(&mut self) {
        self.node.take();
        self.containers.take();
        self.server.abort();
        let result = self.runtime.block_on(async {
            self.pool.close().await;
            sqlx::query(&format!(
                "DROP DATABASE {} WITH (FORCE)",
                self.database_name
            ))
            .execute(&self.admin)
            .await?;
            self.admin.close().await;
            Ok::<_, anyhow::Error>(())
        });
        if let Err(error) = result {
            eprintln!("model database cleanup failed: {error}");
        }
    }
}

#[test]
#[ignore = "requires Postgres"]
fn later_slot_requires_genesis() -> Result<()> {
    let mut driver = ModelDriver::new()?;
    let runtime = Arc::clone(&driver.runtime);
    runtime.block_on(async {
        driver
            .reset(Layout {
                fork_point: 2,
                ..Layout::default()
            })
            .await?;
        let node = driver.node()?;
        let fetched = fetch_batch(node, &[2])
            .await?
            .into_iter()
            .next()
            .context("missing slot")?;
        ensure!(
            process_fetched_slot(node, fetched).await.is_err(),
            "slot after genesis was accepted without a predecessor"
        );
        ensure!(
            node.state_log.last_processed_slot().await?.is_none(),
            "a slot was published without genesis"
        );
        ensure!(
            node.state_log.head_state().await?.is_none(),
            "a DON state was published without genesis"
        );
        Ok(())
    })
}

#[quint_run(spec = "model/reorgs.qnt", max_samples = 100, max_steps = 40)]
#[ignore = "requires Postgres and quint on PATH"]
fn quint_model_generated_traces() -> impl Driver {
    ModelDriver::new().expect("model driver setup failed")
}

macro_rules! scenarios {
    ($($name:ident => $scenario:literal),* $(,)?) => {
        $(
            #[quint_test(spec = "model/reorgs.test.qnt", test = $scenario, max_samples = 1)]
            #[ignore = "requires Postgres and quint on PATH"]
            fn $name() -> impl Driver {
                ModelDriver::new().expect("model driver setup failed")
            }
        )*
    };
}

scenarios! {
    quint_model_reorg_replay => "skippedSlotForkSwitchRewindsTest",
    quint_model_fork_specific_gaps => "forkSpecificGapsTest",
    quint_model_genesis_restart => "genesisRestartTest",
    quint_model_genesis_rewind => "genesisRewindTest",
    quint_model_progress_without_state => "progressWithoutStateTest",
    quint_model_shared_prefix_rewind => "sharedPrefixRewindTest",
}

mod publication;
