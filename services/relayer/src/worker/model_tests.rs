use super::*;

use crate::{
    eth::{FeeEstimate, PendingTxFees},
    time_utils::with_model_time,
};
use alloy::primitives::B256;
use anyhow::{ensure, Context};
use async_trait::async_trait;
use pod2::middleware::{hash_values, Value};
use quint_connect::{quint_run, quint_test, switch, Config, Driver, Step};
use serde::Deserialize;
use sqlx::PgPool;
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};
use tokio::runtime::Runtime;

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(tag = "tag")]
enum RpcBehavior {
    #[default]
    RespondNormally,
    RejectSendOrFailReceipt,
    LoseSendResponse,
    InterruptAcceptedSend,
    FailNonceOrFeeLookup,
    RefuseBroadcast,
    TransportUnavailable,
    InterruptBeforeSend,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(tag = "tag")]
enum TransactionOutcome {
    Prepared,
    Pending,
    Succeeded,
    Reverted,
    Dropped,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
struct Transaction {
    hash: u32,
    job: u32,
    nonce: u64,
    outcome: TransactionOutcome,
}

#[derive(Default)]
struct Chain {
    transactions: Vec<Transaction>,
    behavior: RpcBehavior,
}

struct PreparedTransaction {
    tx_hash: String,
    nonce: u64,
    raw_transaction: Vec<u8>,
}

struct Gateway(Arc<Mutex<Chain>>, Arc<tokio::sync::Notify>);

fn hash_text(hash: u32) -> String {
    format!("{:#x}", B256::repeat_byte(hash as u8))
}

fn hash_id(hash: &str) -> Result<u32> {
    Ok(u32::from(hash.parse::<B256>()?[0]))
}

impl Gateway {
    fn prepare(&self, payload: &[u8], nonce: u64) -> Result<PreparedTransaction> {
        let mut chain = self.0.lock().unwrap();
        if chain.behavior == RpcBehavior::RejectSendOrFailReceipt {
            anyhow::bail!("model preparation rejected");
        }
        let hash = chain.transactions.len() as u32 + 1;
        chain.transactions.push(Transaction {
            hash,
            job: u32::from(payload[0]),
            nonce,
            outcome: TransactionOutcome::Prepared,
        });
        Ok(PreparedTransaction {
            tx_hash: hash_text(hash),
            nonce,
            raw_transaction: vec![hash as u8, nonce as u8, payload[0]],
        })
    }

    async fn broadcast_transaction(&self, prepared: &PreparedTransaction) -> Result<()> {
        let hash = hash_id(&prepared.tx_hash)?;
        ensure!(
            prepared.raw_transaction.len() == 3
                && u32::from(prepared.raw_transaction[0]) == hash
                && u64::from(prepared.raw_transaction[1]) == prepared.nonce,
            "prepared transaction identity changed"
        );
        if self.0.lock().unwrap().behavior == RpcBehavior::InterruptBeforeSend {
            self.1.notify_one();
            std::future::pending::<()>().await;
        }
        let behavior = {
            let mut chain = self.0.lock().unwrap();
            if chain.behavior == RpcBehavior::RefuseBroadcast {
                anyhow::bail!("model JSON-RPC refusal");
            }
            if chain.behavior == RpcBehavior::TransportUnavailable {
                anyhow::bail!("model transport unavailable");
            }
            if chain.transactions.iter().any(|transaction| {
                transaction.nonce == prepared.nonce
                    && transaction.hash != hash
                    && matches!(
                        transaction.outcome,
                        TransactionOutcome::Succeeded | TransactionOutcome::Reverted
                    )
            }) {
                anyhow::bail!("model nonce already consumed");
            }
            if let Some(transaction) = chain
                .transactions
                .iter_mut()
                .find(|transaction| transaction.hash == hash)
            {
                ensure!(
                    transaction.nonce == prepared.nonce
                        && transaction.job == u32::from(prepared.raw_transaction[2]),
                    "rebroadcast changed payload or nonce"
                );
                if matches!(
                    transaction.outcome,
                    TransactionOutcome::Prepared | TransactionOutcome::Dropped
                ) {
                    transaction.outcome = TransactionOutcome::Pending;
                }
            } else {
                chain.transactions.push(Transaction {
                    hash,
                    job: u32::from(prepared.raw_transaction[2]),
                    nonce: prepared.nonce,
                    outcome: TransactionOutcome::Pending,
                });
            }
            chain.behavior
        };
        if behavior == RpcBehavior::InterruptAcceptedSend {
            self.1.notify_one();
            std::future::pending::<()>().await;
        }
        if behavior == RpcBehavior::LoseSendResponse {
            anyhow::bail!("model accepted broadcast response lost");
        }
        Ok(())
    }
}

#[async_trait]
impl EthGateway for Gateway {
    async fn submit_payload(&self, payload: &[u8], nonce: u64) -> Result<String> {
        let prepared = self.prepare(payload, nonce)?;
        self.broadcast_transaction(&prepared).await?;
        Ok(prepared.tx_hash)
    }

    async fn submit_payload_with_fees(
        &self,
        payload: &[u8],
        nonce: u64,
        _: &FeeOverrides,
    ) -> Result<String> {
        self.submit_payload(payload, nonce).await
    }

    async fn get_next_nonce(&self) -> Result<u64> {
        let chain = self.0.lock().unwrap();
        if chain.behavior == RpcBehavior::FailNonceOrFeeLookup {
            anyhow::bail!("model nonce unavailable");
        }
        Ok((0..=10)
            .find(|nonce| {
                !chain.transactions.iter().any(|tx| {
                    tx.nonce == *nonce
                        && !matches!(
                            tx.outcome,
                            TransactionOutcome::Prepared | TransactionOutcome::Dropped
                        )
                })
            })
            .unwrap_or(11))
    }

    async fn poll_receipt(&self, hash: &str) -> Result<Option<ReceiptOutcome>> {
        let chain = self.0.lock().unwrap();
        if chain.behavior == RpcBehavior::RejectSendOrFailReceipt {
            anyhow::bail!("model receipt RPC unavailable");
        }
        let hash = hash_id(hash)?;
        Ok(chain
            .transactions
            .iter()
            .find(|tx| {
                tx.hash == hash
                    && matches!(
                        tx.outcome,
                        TransactionOutcome::Succeeded | TransactionOutcome::Reverted
                    )
            })
            .map(|tx| ReceiptOutcome {
                success: tx.outcome == TransactionOutcome::Succeeded,
                block_number: Some(100 + u64::from(tx.hash)),
            }))
    }

    async fn get_current_fees(&self) -> Result<FeeEstimate> {
        if self.0.lock().unwrap().behavior == RpcBehavior::FailNonceOrFeeLookup {
            anyhow::bail!("model fee RPC unavailable");
        }
        Ok(FeeEstimate {
            base_fee_per_gas: 100,
            max_priority_fee_per_gas: 10,
        })
    }

    async fn get_pending_tx_fees(&self, hash: &str) -> Result<Option<PendingTxFees>> {
        let chain = self.0.lock().unwrap();
        let hash = hash_id(hash)?;
        let pending = chain
            .transactions
            .iter()
            .find(|tx| tx.hash == hash && tx.outcome == TransactionOutcome::Pending)
            .is_some_and(|tx| {
                !chain.transactions.iter().any(|other| {
                    other.nonce == tx.nonce
                        && other.hash > tx.hash
                        && other.outcome == TransactionOutcome::Pending
                })
            });
        Ok(pending.then_some(PendingTxFees {
            max_fee_per_gas: 120,
            max_priority_fee_per_gas: 10,
            max_fee_per_blob_gas: Some(1),
        }))
    }
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct JobView {
    exists: bool,
    status: String,
    attempts: u32,
    nonce: i64,
    current: u32,
    known: BTreeSet<u32>,
    submitted_at: i64,
    bumps: i32,
    next_attempt_at: i64,
    pending: bool,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct ModelState {
    jobs: Vec<JobView>,
    transactions: Vec<Transaction>,
}

struct ModelDriver {
    runtime: Runtime,
    db: Option<Db>,
    pool: PgPool,
    admin_url: String,
    database: String,
    chain: Arc<Mutex<Chain>>,
    now: i64,
    timeout_enabled: bool,
}

impl ModelDriver {
    fn new() -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let (db, admin_url, database) = runtime
            .block_on(super::tests::setup_db())
            .expect("relayer model database setup failed");
        let mut url = url::Url::parse(&admin_url).unwrap();
        url.set_path(&format!("/{database}"));
        let pool = runtime.block_on(PgPool::connect(url.as_str())).unwrap();
        Self {
            runtime,
            db: Some(db),
            pool,
            admin_url,
            database,
            chain: Arc::new(Mutex::new(Chain::default())),
            now: 0,
            timeout_enabled: true,
        }
    }

    fn db(&self) -> &Db {
        self.db.as_ref().unwrap()
    }

    fn cfg() -> WorkerConfig {
        WorkerConfig {
            max_attempts: 2,
            retry_initial_secs: 1,
            retry_max_secs: 2,
            receipt_poll_secs: 1,
            receipt_timeout_secs: Some(8),
            idle_sleep_ms: 0,
            fee_bump_after_secs: Some(2),
            fee_bump_multiplier_pct: 20,
            fee_bump_max: 2,
        }
    }

    fn enqueue(&self, id: u32) -> Result<()> {
        let job = RelayJob {
            job_id: id.to_string(),
            status: JobStatus::Queued,
            payload_bytes: vec![id as u8],
            tx_final: hash_values(&[Value::from(i64::from(id))]),
            state_root: hash_values(&[]),
            client_ref: None,
            attempt_count: 0,
            tx_hash: None,
            submitted_at: None,
            block_number: None,
            last_error: None,
            next_attempt_at: Some(self.now),
            nonce: None,
            bump_count: 0,
            prev_tx_hashes: vec![],
            created_at: self.now,
            updated_at: self.now,
        };
        self.runtime.block_on(self.db().insert_job(&job))?;
        Ok(())
    }

    fn process(&self, id: u32, behavior: RpcBehavior) -> Result<()> {
        self.chain.lock().unwrap().behavior = behavior;
        let job = self
            .runtime
            .block_on(self.db().get_job(&id.to_string()))?
            .context("job absent")?;
        let interrupted = Arc::new(tokio::sync::Notify::new());
        let gateway = Gateway(Arc::clone(&self.chain), Arc::clone(&interrupted));
        let mut cfg = Self::cfg();
        if !self.timeout_enabled {
            cfg.receipt_timeout_secs = None;
        }
        let result = with_model_time(self.now, || {
            self.runtime.block_on(async {
                tokio::select! {
                    result = process_due_job(self.db(), &gateway, &cfg, job) => result,
                    _ = interrupted.notified() => {
                        ensure!(
                            matches!(behavior, RpcBehavior::InterruptBeforeSend | RpcBehavior::InterruptAcceptedSend),
                            "unexpected worker interruption"
                        );
                        Ok(())
                    },
                }
            })
        });
        if let Err(error) = result {
            ensure!(
                behavior == RpcBehavior::FailNonceOrFeeLookup
                    && error.to_string().contains("model nonce unavailable"),
                "unexpected worker error: {error}"
            );
        }
        Ok(())
    }

    fn pending(&self, job: &RelayJob) -> Result<bool> {
        let table: Option<String> = self.runtime.block_on(
            sqlx::query_scalar("SELECT to_regclass('relay_pending_broadcasts')::text")
                .fetch_one(&self.pool),
        )?;
        if table.is_none() {
            return Ok(false);
        }
        let raw: Option<Vec<u8>> = self.runtime.block_on(
            sqlx::query_scalar(
                "SELECT raw_transaction FROM relay_pending_broadcasts WHERE job_id = $1",
            )
            .bind(&job.job_id)
            .fetch_optional(&self.pool),
        )?;
        if let Some(raw) = raw {
            let hash = hash_id(
                job.tx_hash
                    .as_deref()
                    .context("pending send has no identity")?,
            )?;
            let nonce = u8::try_from(job.nonce.context("pending send has no nonce")?)?;
            ensure!(
                raw == [u8::try_from(hash)?, nonce, job.payload_bytes[0]],
                "persisted transaction bytes changed identity, nonce or payload"
            );
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn restart(&mut self) -> Result<()> {
        self.runtime
            .block_on(self.db().recover_inflight_jobs(self.now))?;
        self.db.take();
        let mut url = url::Url::parse(&self.admin_url)?;
        url.set_path(&format!("/{}", self.database));
        self.db = Some(self.runtime.block_on(Db::connect(url.as_str()))?);
        Ok(())
    }

    fn mine(&self, hash: u32, success: bool) {
        let mut chain = self.chain.lock().unwrap();
        let nonce = chain.transactions[(hash - 1) as usize].nonce;
        for transaction in &mut chain.transactions {
            if transaction.nonce == nonce && transaction.outcome != TransactionOutcome::Prepared {
                transaction.outcome = if transaction.hash != hash {
                    TransactionOutcome::Dropped
                } else if success {
                    TransactionOutcome::Succeeded
                } else {
                    TransactionOutcome::Reverted
                };
            }
        }
    }
}

impl quint_connect::State<ModelDriver> for ModelState {
    fn from_driver(driver: &ModelDriver) -> Result<Self> {
        let mut jobs = Vec::new();
        for id in 0..2 {
            let job = driver
                .runtime
                .block_on(driver.db().get_job(&id.to_string()))?;
            let view = if let Some(job) = job {
                let current = job
                    .tx_hash
                    .as_deref()
                    .map(hash_id)
                    .transpose()?
                    .unwrap_or(0);
                let known = job
                    .prev_tx_hashes
                    .iter()
                    .chain(job.tx_hash.iter())
                    .map(|hash| hash_id(hash))
                    .collect::<Result<BTreeSet<_>>>()?;
                JobView {
                    exists: true,
                    status: job.status.as_str().into(),
                    attempts: job.attempt_count,
                    nonce: job.nonce.unwrap_or(-1),
                    current,
                    known,
                    submitted_at: job.submitted_at.unwrap_or(-1),
                    bumps: job.bump_count,
                    next_attempt_at: job.next_attempt_at.unwrap_or(-1),
                    pending: driver.pending(&job)?,
                }
            } else {
                JobView {
                    exists: false,
                    status: "absent".into(),
                    attempts: 0,
                    nonce: -1,
                    current: 0,
                    known: BTreeSet::new(),
                    submitted_at: -1,
                    bumps: 0,
                    next_attempt_at: -1,
                    pending: false,
                }
            };
            jobs.push(view);
        }
        Ok(Self {
            jobs,
            transactions: driver.chain.lock().unwrap().transactions.clone(),
        })
    }
}

impl Driver for ModelDriver {
    type State = ModelState;

    fn config() -> Config {
        Config {
            state: &["state"],
            nondet: &["command"],
        }
    }

    fn step(&mut self, step: &Step) -> Result<()> {
        switch!(step {
            Init(timeout_enabled: bool) => {
                self.timeout_enabled = timeout_enabled;
                self.runtime
                    .block_on(sqlx::query("TRUNCATE relay_jobs CASCADE").execute(&self.pool))?;
                *self.chain.lock().unwrap() = Chain::default();
                self.now = 0;
            },
            Enqueue(job: u32) => self.enqueue(job)?,
            Process(job: u32, behavior: RpcBehavior) => self.process(job, behavior)?,
            PollIdle(behavior: RpcBehavior) => {
                if let Some(job) = self.runtime.block_on(self.db().next_due_job(self.now))? {
                    self.process(job.job_id.parse()?, behavior)?;
                }
            },
            Tick => self.now += 1,
            Restart => self.restart()?,
            Mine(hash: u32, success: bool) => self.mine(hash, success),
            Drop(hash: u32) => {
                self.chain.lock().unwrap().transactions[(hash - 1) as usize].outcome =
                    TransactionOutcome::Dropped;
            },
        })
    }
}

impl Drop for ModelDriver {
    fn drop(&mut self) {
        self.db.take();
        self.runtime
            .block_on(async {
                self.pool.close().await;
                super::tests::drop_db(&self.admin_url, &self.database).await
            })
            .expect("relayer model database cleanup failed");
    }
}

#[quint_run(spec = "model/delivery.qnt", max_samples = 100, max_steps = 45)]
#[ignore = "requires Postgres and quint on PATH"]
fn quint_model_delivery_generated() -> impl Driver {
    ModelDriver::new()
}

macro_rules! scenarios {
    ($($name:ident => $scenario:literal),* $(,)?) => {
        $(
            #[quint_test(spec = "model/delivery.qnt", test = $scenario, max_samples = 1)]
            #[ignore = "requires Postgres and quint on PATH"]
            fn $name() -> impl Driver {
                ModelDriver::new()
            }
        )*
    };
}

scenarios! {
    quint_model_delivery_receipt_rpc => "receiptRpcFailureTest",
    quint_model_delivery_old_receipt => "oldReplacementMinedTest",
    quint_model_delivery_ambiguous_restart => "ambiguousSendRestartTest",
    quint_model_delivery_nonce_backoff => "nonceFailureBackoffTest",
    quint_model_delivery_nonce_recovery => "nonceFailureRecoveryTest",
    quint_model_delivery_late_receipt => "receiptAfterDeadlineTest",
    quint_model_delivery_receipt_outage => "receiptOutagePastDeadlineTest",
    quint_model_delivery_late_replacement => "replacementReceiptAfterDeadlineTest",
    quint_model_delivery_reserved_nonce => "pendingNonceReservationTest",
    quint_model_delivery_consumed_nonce => "nonceConsumedBeforeRebroadcastTest",
    quint_model_delivery_initial_refusal => "initialRefusalWithoutTimeoutTest",
    quint_model_delivery_refusal_releases_nonce => "refusalReleasesNonceTest",
    quint_model_delivery_replacement_refusal => "replacementRefusalTest",
    quint_model_delivery_resend_refusal => "resendRefusalRemainsAmbiguousTest",
    quint_model_delivery_transport_ambiguity => "transportFailureRemainsAmbiguousTest",
    quint_model_delivery_before_initial_send => "crashBeforeInitialSendTest",
    quint_model_delivery_before_replacement_send => "crashBeforeReplacementSendTest",
    quint_model_delivery_consumed_nonce_prevents_interruption => "consumedNoncePreventsInterruptionTest",
}
