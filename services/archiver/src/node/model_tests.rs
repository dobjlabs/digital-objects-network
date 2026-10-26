use super::*;
use anyhow::{ensure, Context};
use axum::{
    extract::{Path as RoutePath, RawQuery, State},
    routing::{get, post},
    Json, Router,
};
use quint_connect::{quint_run, quint_test, switch, Config as ConnectConfig, Driver, Step};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, RwLock},
    time::Duration,
};
use tempfile::TempDir;
use tokio::{
    runtime::Runtime,
    sync::{mpsc, Semaphore},
    task::JoinHandle,
};

#[derive(Debug)]
pub(super) enum Event {
    Linked,
    HeaderPrepared,
    BlobStored { root: B256, index: u32 },
    PayloadReady,
    Published,
}

pub(super) struct Gate {
    sender: mpsc::UnboundedSender<Event>,
    permits: Semaphore,
}

impl Gate {
    fn new() -> (Arc<Self>, mpsc::UnboundedReceiver<Event>) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (
            Arc::new(Self {
                sender,
                permits: Semaphore::new(0),
            }),
            receiver,
        )
    }

    pub(super) async fn pause(&self, event: Event) {
        self.sender
            .send(event)
            .expect("archive event receiver closed");
        self.permits
            .acquire()
            .await
            .expect("archive gate closed")
            .forget();
    }
}

#[derive(Default)]
struct Source {
    withheld: BTreeSet<u32>,
    base_slot: u32,
}

fn root_hash(root: u32) -> B256 {
    B256::repeat_byte(root as u8)
}

fn root_id(root: B256) -> u32 {
    u32::from(root[0])
}

fn parent_root(root: u32) -> u32 {
    if root == 11 {
        1
    } else if root % 10 == 2 {
        11
    } else {
        root - 1
    }
}

fn commitment(root: u32, index: u32) -> KzgCommitment {
    KzgCommitment::repeat_byte((root * 2 + index) as u8)
}

fn versioned_hash(root: u32, index: u32) -> B256 {
    kzg_to_versioned_hash(commitment(root, index).as_ref())
}

fn blob(root: u32, index: u32) -> HeapBlob {
    let mut bytes = vec![0; 131072];
    bytes[0] = root as u8;
    bytes[1] = index as u8;
    HeapBlob::from_bytes(Bytes::from(bytes)).unwrap()
}

fn transaction_json(root: u32) -> Value {
    json!({ "type":"0x3", "chainId":"0x1", "nonce":"0x0", "gas":"0x5208",
        "maxFeePerGas":"0x64", "maxPriorityFeePerGas":"0x1", "maxFeePerBlobGas":"0x1",
        "to": alloy::primitives::Address::ZERO, "value":"0x0", "input":"0x", "accessList":[],
        "blobVersionedHashes":[versioned_hash(root,0),versioned_hash(root,1)],
        "yParity":"0x0", "v":"0x0", "r":"0x1", "s":"0x1",
        "hash":root_hash(root), "from":alloy::primitives::Address::ZERO,
        "blockHash":root_hash(root), "blockNumber":"0x1", "transactionIndex":"0x0" })
}

async fn get_block(
    State(source): State<Arc<RwLock<Source>>>,
    RoutePath(root): RoutePath<B256>,
) -> Json<Value> {
    let root = root_id(root);
    let source = source.read().unwrap();
    let parent = if root == 1 { 0 } else { parent_root(root) };
    let slot = if root == 1 {
        source.base_slot - 1
    } else {
        source.base_slot + root % 10 - 1
    };
    let commits: Vec<_> = if root == 1 {
        vec![]
    } else {
        (0..2).map(|index| commitment(root, index)).collect()
    };
    Json(
        json!({ "data": { "message": { "slot":slot.to_string(), "parent_root":root_hash(parent),
        "body": { "signed_execution_payload_bid": { "message": {
            "block_hash":root_hash(root),
            "parent_block_hash": if source.withheld.contains(&root) {
                B256::ZERO
            } else {
                root_hash(parent)
            },
            "blob_kzg_commitments": commits,
        }}} }}}),
    )
}

async fn get_blobs(RoutePath(root): RoutePath<B256>, RawQuery(query): RawQuery) -> Json<Value> {
    let root = root_id(root);
    let requested: BTreeSet<B256> =
        url::form_urlencoded::parse(query.unwrap_or_default().as_bytes())
            .filter(|(key, _)| key == "versioned_hashes")
            .map(|(_, value)| value.parse().unwrap())
            .collect();
    let blobs: Vec<_> = (0..2)
        .filter(|index| requested.contains(&versioned_hash(root, *index)))
        .map(|index| blob(root, index))
        .collect();
    Json(json!({ "data":blobs }))
}

async fn execution_rpc(Json(request): Json<Value>) -> Json<Value> {
    let hash: B256 = serde_json::from_value(request["params"][0].clone()).unwrap();
    let root = root_id(hash);
    let mut block = alloy::rpc::types::Block::<alloy::rpc::types::Transaction>::default();
    block.header.hash = hash;
    block.header.inner.number = u64::from(root % 10);
    let mut block = serde_json::to_value(block).unwrap();
    block["transactions"] = json!([transaction_json(root)]);
    Json(json!({ "jsonrpc":"2.0", "id":request["id"], "result":block }))
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(tag = "tag")]
enum AcquisitionPhase {
    Idle,
    Linked,
    HeaderPrepared,
    BlobStored,
    PayloadReady,
    Published,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct ModelState {
    committed: Vec<u32>,
    phase: AcquisitionPhase,
    blobs: BTreeMap<u32, BTreeSet<u32>>,
}

struct ModelDriver {
    runtime: Arc<Runtime>,
    source: Arc<RwLock<Source>>,
    server: JoinHandle<()>,
    base_url: String,
    storage: TempDir,
    node: Option<Arc<Node>>,
    fork: u32,
    phase: AcquisitionPhase,
    pending: u32,
    gate: Option<Arc<Gate>>,
    events: Option<mpsc::UnboundedReceiver<Event>>,
    task: Option<JoinHandle<Result<()>>>,
}

impl ModelDriver {
    fn new() -> Self {
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap(),
        );
        let source = Arc::new(RwLock::new(Source::default()));
        let (server, base_url) = runtime.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base_url = format!("http://{}", listener.local_addr().unwrap());
            let router = Router::new()
                .route("/eth/v2/beacon/blocks/{root}", get(get_block))
                .route("/eth/v1/beacon/blobs/{root}", get(get_blobs))
                .route("/rpc", post(execution_rpc))
                .with_state(Arc::clone(&source));
            let server = tokio::spawn(async move {
                axum::serve(listener, router).await.unwrap();
            });
            (server, base_url)
        });
        Self {
            runtime,
            source,
            server,
            base_url,
            storage: TempDir::new().unwrap(),
            node: None,
            fork: 1,
            phase: AcquisitionPhase::Idle,
            pending: 0,
            gate: None,
            events: None,
            task: None,
        }
    }

    fn node(&self) -> &Arc<Node> {
        self.node.as_ref().unwrap()
    }

    async fn abort(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
            let _ = task.await;
        }
        if let Some(node) = &self.node {
            *node.store.model_gate.lock().unwrap() = None;
        }
        self.phase = AcquisitionPhase::Idle;
        self.pending = 0;
        self.gate = None;
        self.events = None;
    }

    async fn init(&mut self, withheld: BTreeSet<u32>, base_slot: u32) -> Result<()> {
        self.abort().await;
        self.node.take();
        let path = self.storage.path().join("archive");
        if path.exists() {
            fs::remove_dir_all(&path)?;
        }
        *self.source.write().unwrap() = Source {
            withheld,
            base_slot,
        };
        self.fork = 1;
        let _: alloy::rpc::types::Transaction = serde_json::from_value(transaction_json(11))?;
        self.node = Some(Arc::new(
            Node::new(crate::config::Config {
                blobs_path: path.to_string_lossy().into_owned(),
                http_bind: "127.0.0.1:0".parse()?,
                init_start_slot: base_slot,
                rpc_url: format!("{}/rpc", self.base_url),
                beacon_url: self.base_url.clone(),
                filter_address: alloy::primitives::Address::ZERO,
            })
            .await?,
        ));
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        let previous = self.node().store.last_header()?;
        let slot = previous
            .as_ref()
            .map(|header| header.slot + 1)
            .unwrap_or(self.node().config.init_start_slot);
        let position = slot - self.node().config.init_start_slot + 1;
        let root = if position == 1 {
            11
        } else {
            self.fork * 10 + position
        };
        let header = BlockHeader {
            slot,
            root: root_hash(root),
            parent_root: root_hash(parent_root(root)),
        };
        if crate::rewind_on_parent_change(self.node(), slot, &header, previous.as_ref())?.is_some()
        {
            return Ok(());
        }
        let (gate, events) = Gate::new();
        *self.node().store.model_gate.lock().unwrap() = Some(Arc::clone(&gate));
        let node = Arc::clone(self.node());
        self.task = Some(tokio::spawn(async move {
            node.process_beacon_block_header(&header, None).await?;
            Ok(())
        }));
        self.gate = Some(gate);
        self.events = Some(events);
        self.pending = root;
        self.observe().await
    }

    async fn observe(&mut self) -> Result<()> {
        let event = tokio::time::timeout(
            Duration::from_secs(10),
            self.events.as_mut().unwrap().recv(),
        )
        .await?
        .context("archive operation stopped before its next checkpoint")?;
        self.phase = match event {
            Event::Linked => AcquisitionPhase::Linked,
            Event::HeaderPrepared => AcquisitionPhase::HeaderPrepared,
            Event::BlobStored { root, index } => {
                ensure!(
                    root_id(root) == parent_root(self.pending) && index < 2,
                    "wrong payload ownership"
                );
                AcquisitionPhase::BlobStored
            }
            Event::PayloadReady => AcquisitionPhase::PayloadReady,
            Event::Published => AcquisitionPhase::Published,
        };
        Ok(())
    }

    async fn advance(&mut self) -> Result<()> {
        self.gate.as_ref().unwrap().permits.add_permits(1);
        if self.phase == AcquisitionPhase::Published {
            self.task.take().unwrap().await??;
            *self.node().store.model_gate.lock().unwrap() = None;
            self.phase = AcquisitionPhase::Idle;
            self.pending = 0;
            self.gate = None;
            self.events = None;
            Ok(())
        } else {
            self.observe().await
        }
    }

    async fn snapshot(&self) -> Result<ModelState> {
        let base_slot = self.source.read().unwrap().base_slot;
        let mut committed = Vec::new();
        for position in 0..4 {
            let path = self
                .node()
                .store
                .slot_dir(base_slot + position)
                .join("header.json");
            if path.exists() {
                let header: BlockHeader = serde_json::from_slice(&fs::read(path)?)?;
                committed.push(root_id(header.root));
            }
        }
        let mut blobs = BTreeMap::new();
        for root in [11, 12, 13, 14, 22, 23, 24] {
            let mut stored = BTreeSet::new();
            for (index, hash, data) in self.node().store.load_blobs_disk(&root_hash(root)).await? {
                ensure!(
                    index < 2
                        && hash == versioned_hash(root, index as u32)
                        && data.inner() == blob(root, index as u32).inner(),
                    "incorrect archived blob"
                );
                stored.insert(index as u32);
            }
            blobs.insert(root, stored);
        }
        Ok(ModelState {
            committed,
            phase: self.phase,
            blobs,
        })
    }
}

impl quint_connect::State<ModelDriver> for ModelState {
    fn from_driver(driver: &ModelDriver) -> Result<Self> {
        driver.runtime.block_on(driver.snapshot())
    }
}

impl Driver for ModelDriver {
    type State = ModelState;

    fn config() -> ConnectConfig {
        ConnectConfig {
            state: &["state"],
            nondet: &["command"],
        }
    }

    fn step(&mut self, step: &Step) -> Result<()> {
        let runtime = Arc::clone(&self.runtime);
        switch!(step {
            Init(withheld: BTreeSet<u32>, base_slot: u32) => {
                runtime.block_on(self.init(withheld, base_slot))?;
            },
            Start => runtime.block_on(self.start())?,
            Advance => runtime.block_on(self.advance())?,
            SwitchFork => self.fork = 3 - self.fork,
            Crash => {
                runtime.block_on(self.abort());
                self.node().store.last_header()?;
            },
        })
    }
}

impl Drop for ModelDriver {
    fn drop(&mut self) {
        let runtime = Arc::clone(&self.runtime);
        runtime.block_on(self.abort());
        self.server.abort();
    }
}

#[quint_run(spec = "model/acquisition.qnt", max_samples = 60, max_steps = 35)]
#[ignore = "requires quint on PATH"]
fn quint_model_acquisition_generated() -> impl Driver {
    ModelDriver::new()
}

macro_rules! scenarios {
    ($($name:ident => $scenario:literal),* $(,)?) => {
        $(
            #[quint_test(spec = "model/acquisition.qnt", test = $scenario, max_samples = 1)]
            #[ignore = "requires quint on PATH"]
            fn $name() -> impl Driver {
                ModelDriver::new()
            }
        )*
    };
}

scenarios! {
    quint_model_acquisition_partial_parent => "partialParentPayloadRestartTest",
    quint_model_acquisition_published_restart => "publishedRestartTest",
    quint_model_acquisition_withheld => "withheldPayloadTest",
    quint_model_acquisition_reorg => "reorgRetainsAncestorPayloadTest",
}
