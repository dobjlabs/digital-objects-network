use std::{sync::Arc, time::Duration};

use anyhow::{anyhow, Result};
use tokio::sync::watch;
use tracing::{info, warn};

use crate::{
    db::Db,
    eth::{EthGateway, FeeOverrides, ReceiptOutcome},
    model::{JobStatus, RelayJob},
    time_utils::now_ts,
};

/// Runtime knobs controlling retry/poll cadence and failure thresholds.
#[derive(Clone, Debug)]
pub struct WorkerConfig {
    pub max_attempts: u32,
    pub retry_initial_secs: u64,
    pub retry_max_secs: u64,
    pub receipt_poll_secs: u64,
    pub receipt_timeout_secs: Option<u64>,
    pub idle_sleep_ms: u64,
    /// Seconds after submission before first fee-bump attempt. `None` disables bumping.
    pub fee_bump_after_secs: Option<u64>,
    /// Percentage to increase fees per bump (e.g. 20 = 1.2x). Min 13 for EIP-1559 rules.
    pub fee_bump_multiplier_pct: u64,
    /// Maximum number of fee bumps per job.
    pub fee_bump_max: u32,
}

/// Main worker loop: recover inflight rows, then repeatedly pick and process due jobs.
pub async fn run_worker(
    db: Arc<Db>,
    eth_client: Arc<dyn EthGateway>,
    cfg: WorkerConfig,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<()> {
    info!(?cfg, "Relayer worker started");

    let recovered = db.recover_inflight_jobs(now_ts()).await?;
    if recovered > 0 {
        info!(recovered, "Recovered in-flight relay jobs");
    }

    loop {
        if *shutdown_rx.borrow() {
            info!("Relayer worker shutting down");
            return Ok(());
        }

        let now = now_ts();
        let Some(job) = db.next_due_job(now).await? else {
            if wait_or_shutdown(Duration::from_millis(cfg.idle_sleep_ms), &mut shutdown_rx).await {
                info!("Relayer worker shutting down");
                return Ok(());
            }
            continue;
        };

        info!(
            job_id = %job.job_id,
            status = job.status.as_str(),
            attempts = job.attempt_count,
            next_attempt_at = ?job.next_attempt_at,
            tx_hash = ?job.tx_hash,
            "Picked due relay job"
        );
        if let Err(err) = process_due_job(&db, &*eth_client, &cfg, job).await {
            warn!(?err, "Processing relay job failed unexpectedly");
        }
    }
}

/// Sleep for `duration` unless shutdown is requested first.
async fn wait_or_shutdown(duration: Duration, shutdown_rx: &mut watch::Receiver<bool>) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(duration) => false,
        changed = shutdown_rx.changed() => {
            if changed.is_err() {
                return true;
            }
            *shutdown_rx.borrow()
        }
    }
}

/// Route a due job to submit-path or receipt-poll path.
async fn process_due_job(
    db: &Db,
    eth_client: &dyn EthGateway,
    cfg: &WorkerConfig,
    job: RelayJob,
) -> Result<()> {
    if job.status.is_terminal() {
        return Ok(());
    }

    if job.tx_hash.is_some() {
        poll_submitted_job(db, eth_client, cfg, job).await
    } else {
        send_queued_job(db, eth_client, cfg, job).await
    }
}

/// Submit a queued payload as an EIP-4844 transaction and update job state accordingly.
async fn send_queued_job(
    db: &Db,
    eth_client: &dyn EthGateway,
    cfg: &WorkerConfig,
    mut job: RelayJob,
) -> Result<()> {
    let now = now_ts();
    job.status = JobStatus::Sending;
    job.attempt_count = job.attempt_count.saturating_add(1);
    job.updated_at = now;
    db.put_job(&job).await?;

    // Query nonce before submission so we can store it for potential fee bumps.
    // Single-worker guarantees no nonce race between query and send_transaction.
    let nonce = match eth_client.get_next_nonce().await {
        Ok(n) => Some(n),
        Err(err) => {
            warn!(job_id = %job.job_id, ?err, "Failed to query nonce; submitting without tracking");
            None
        }
    };

    info!(
        job_id = %job.job_id,
        attempt = job.attempt_count,
        payload_bytes = job.payload_bytes.len(),
        tx_final = %job.tx_final,
        state_root_hash = %job.state_root_hash,
        nonce = ?nonce,
        "Submitting relay payload to Ethereum"
    );

    match eth_client.submit_payload(&job.payload_bytes).await {
        Ok(tx_hash) => {
            job.status = JobStatus::Submitted;
            job.tx_hash = Some(tx_hash);
            job.submitted_at = Some(now);
            job.next_attempt_at = Some(now + cfg.receipt_poll_secs as i64);
            job.last_error = None;
            job.nonce = nonce.map(|n| n as i64);
            job.bump_count = 0;
            job.updated_at = now;
            db.put_job(&job).await?;
            info!(
                job_id = %job.job_id,
                tx_hash = ?job.tx_hash,
                nonce = ?job.nonce,
                next_attempt_at = ?job.next_attempt_at,
                "Submitted blob transaction"
            );
            Ok(())
        }
        Err(err) => {
            schedule_retry_or_fail(db, cfg, job, now, err).await?;
            Ok(())
        }
    }
}

/// Poll receipt for a previously submitted tx and transition job state.
async fn poll_submitted_job(
    db: &Db,
    eth_client: &dyn EthGateway,
    cfg: &WorkerConfig,
    mut job: RelayJob,
) -> Result<()> {
    let now = now_ts();
    let tx_hash_str = job
        .tx_hash
        .clone()
        .ok_or_else(|| anyhow!("submitted job missing tx_hash"))?;

    if let (Some(timeout_secs), Some(submitted_at)) = (cfg.receipt_timeout_secs, job.submitted_at) {
        if now.saturating_sub(submitted_at) >= timeout_secs as i64 {
            warn!(
                job_id = %job.job_id,
                tx_hash = %tx_hash_str,
                timeout_secs = timeout_secs,
                elapsed_secs = now.saturating_sub(submitted_at),
                "Receipt polling timeout reached"
            );
            let block_number = job.block_number;
            fail_job_with_block(
                db,
                &mut job,
                now,
                block_number,
                format!("receipt timeout after {}s", timeout_secs),
            )
            .await?;
            return Ok(());
        }
    }

    job.updated_at = now;
    db.put_job(&job).await?;

    info!(
        job_id = %job.job_id,
        tx_hash = %tx_hash_str,
        attempt = job.attempt_count,
        "Polling transaction receipt"
    );

    match eth_client.poll_receipt(&tx_hash_str).await {
        Ok(Some(ReceiptOutcome {
            success: true,
            block_number,
        })) => {
            job.status = JobStatus::Confirmed;
            job.block_number = block_number;
            job.next_attempt_at = None;
            job.last_error = None;
            job.updated_at = now;
            db.put_job(&job).await?;
            info!(
                job_id = %job.job_id,
                tx_hash = %tx_hash_str,
                block_number = ?job.block_number,
                attempts = job.attempt_count,
                "Relay job confirmed"
            );
            Ok(())
        }
        Ok(Some(ReceiptOutcome {
            success: false,
            block_number,
        })) => {
            fail_job_with_block(
                db,
                &mut job,
                now,
                block_number,
                "transaction reverted on-chain".to_string(),
            )
            .await?;
            warn!(
                job_id = %job.job_id,
                tx_hash = %tx_hash_str,
                block_number = ?block_number,
                "Relay job failed due to reverted receipt"
            );
            Ok(())
        }
        Ok(None) => {
            // Check if we should attempt a fee bump.
            if let Some(bump_after) = cfg.fee_bump_after_secs {
                if let (Some(submitted_at), Some(nonce)) = (job.submitted_at, job.nonce) {
                    let bump_threshold = bump_after as i64 * (job.bump_count as i64 + 1);
                    let elapsed = now.saturating_sub(submitted_at);

                    if elapsed >= bump_threshold && (job.bump_count as u32) < cfg.fee_bump_max {
                        match try_fee_bump(
                            eth_client,
                            cfg,
                            &job.payload_bytes,
                            nonce as u64,
                            &tx_hash_str,
                            job.bump_count,
                        )
                        .await
                        {
                            Ok(new_tx_hash) => {
                                let old_hash = tx_hash_str.clone();
                                job.tx_hash = Some(new_tx_hash.clone());
                                job.bump_count += 1;
                                job.next_attempt_at = Some(now + cfg.receipt_poll_secs as i64);
                                job.updated_at = now;
                                db.put_job(&job).await?;
                                info!(
                                    job_id = %job.job_id,
                                    old_tx_hash = %old_hash,
                                    new_tx_hash = %new_tx_hash,
                                    bump_count = job.bump_count,
                                    "Fee-bumped blob transaction"
                                );
                                return Ok(());
                            }
                            Err(err) => {
                                warn!(
                                    job_id = %job.job_id,
                                    bump_count = job.bump_count,
                                    ?err,
                                    "Fee bump failed; continuing to poll original tx"
                                );
                            }
                        }
                    }
                }
            }

            job.status = JobStatus::Submitted;
            job.next_attempt_at = Some(now + cfg.receipt_poll_secs as i64);
            job.updated_at = now;
            db.put_job(&job).await?;
            info!(
                job_id = %job.job_id,
                tx_hash = %tx_hash_str,
                next_attempt_at = ?job.next_attempt_at,
                "Receipt not ready yet; will poll again"
            );
            Ok(())
        }
        Err(err) => {
            if is_permanent_error(&err) {
                warn!(
                    job_id = %job.job_id,
                    tx_hash = %tx_hash_str,
                    error = %err,
                    "Permanent receipt polling error; marking failed"
                );
                let block_number = job.block_number;
                fail_job_with_block(db, &mut job, now, block_number, err.to_string()).await?;
                return Ok(());
            }
            schedule_retry_or_fail(db, cfg, job, now, err).await?;
            Ok(())
        }
    }
}

/// Fetch the original TX's fees and current network fees, take the max of each,
/// apply the bump multiplier, and resubmit at the same nonce.
async fn try_fee_bump(
    eth_client: &dyn EthGateway,
    cfg: &WorkerConfig,
    payload_bytes: &[u8],
    nonce: u64,
    current_tx_hash: &str,
    current_bump_count: i32,
) -> Result<String> {
    let network = eth_client.get_current_fees().await?;

    // Fetch the queued TX's fee caps so we can guarantee the replacement exceeds them.
    let original = eth_client
        .get_pending_tx_fees(current_tx_hash)
        .await?
        .ok_or_else(|| anyhow!("pending tx {current_tx_hash} not found in mempool"))?;

    let multiplier_pct = cfg.fee_bump_multiplier_pct;
    let apply_bump =
        |base: u128| -> u128 { base.saturating_mul(100 + multiplier_pct as u128) / 100 };

    // Use max(network estimate, original TX) as the floor, then bump above it.
    let base_priority = original
        .max_priority_fee_per_gas
        .max(network.max_priority_fee_per_gas);
    let base_max_fee = original
        .max_fee_per_gas
        .max(network.base_fee_per_gas + base_priority);

    let bumped_priority = apply_bump(base_priority);
    let bumped_max_fee = apply_bump(base_max_fee);

    // Bump blob fee relative to the original TX's blob fee — not the current
    // network rate, which can be orders of magnitude higher on some chains.
    let bumped_blob_fee = original.max_fee_per_blob_gas.map(apply_bump);

    let fees = FeeOverrides {
        max_fee_per_gas: bumped_max_fee,
        max_priority_fee_per_gas: bumped_priority,
        max_fee_per_blob_gas: bumped_blob_fee,
    };

    info!(
        nonce,
        bump = current_bump_count + 1,
        max_fee_per_gas = fees.max_fee_per_gas,
        max_priority_fee_per_gas = fees.max_priority_fee_per_gas,
        max_fee_per_blob_gas = ?fees.max_fee_per_blob_gas,
        original_max_fee = original.max_fee_per_gas,
        original_blob_fee = ?original.max_fee_per_blob_gas,
        network_base_fee = network.base_fee_per_gas,
        "Attempting fee bump"
    );

    eth_client
        .submit_payload_with_fees(payload_bytes, nonce, &fees)
        .await
}

/// Mark a job as failed while preserving any known receipt block number.
async fn fail_job_with_block(
    db: &Db,
    job: &mut RelayJob,
    now: i64,
    block_number: Option<u64>,
    reason: String,
) -> Result<()> {
    warn!(
        job_id = %job.job_id,
        block_number = ?block_number,
        reason = %reason,
        "Relay job marked failed"
    );
    job.status = JobStatus::Failed;
    job.block_number = block_number;
    job.next_attempt_at = None;
    job.last_error = Some(reason);
    job.updated_at = now;
    db.put_job(job).await
}

/// Apply exponential backoff retries, or fail permanently if attempts are exhausted.
async fn schedule_retry_or_fail(
    db: &Db,
    cfg: &WorkerConfig,
    mut job: RelayJob,
    now: i64,
    err: anyhow::Error,
) -> Result<()> {
    let msg = err.to_string();
    if job.attempt_count >= cfg.max_attempts {
        job.status = JobStatus::Failed;
        job.next_attempt_at = None;
        job.last_error = Some(msg);
        job.updated_at = now;
        db.put_job(&job).await?;
        warn!(
            job_id = %job.job_id,
            attempts = job.attempt_count,
            max_attempts = cfg.max_attempts,
            error = ?job.last_error,
            "Relay job marked failed after exhausting retries"
        );
        return Ok(());
    }

    let backoff = backoff_secs(
        job.attempt_count,
        cfg.retry_initial_secs,
        cfg.retry_max_secs,
    );
    job.status = JobStatus::Queued;
    job.next_attempt_at = Some(now + backoff as i64);
    job.last_error = Some(msg);
    job.updated_at = now;
    db.put_job(&job).await?;

    warn!(
        job_id = %job.job_id,
        attempts = job.attempt_count,
        retry_in_secs = backoff,
        next_attempt_at = ?job.next_attempt_at,
        error = ?job.last_error,
        "Relay job scheduled for retry"
    );
    Ok(())
}

fn is_permanent_error(err: &anyhow::Error) -> bool {
    err.to_string().contains("invalid tx hash")
}

fn backoff_secs(attempt_count: u32, initial: u64, max: u64) -> u64 {
    let shift = attempt_count.saturating_sub(1).min(20);
    let exp = 1u64 << shift;
    initial.saturating_mul(exp).min(max)
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, sync::Mutex};

    use async_trait::async_trait;
    use sqlx::{postgres::PgPoolOptions, Executor};
    use url::Url;

    use crate::eth::{FeeEstimate, PendingTxFees};

    use super::*;

    #[derive(Default)]
    struct MockEthGateway {
        submit_results: Mutex<VecDeque<Result<String>>>,
        poll_results: Mutex<VecDeque<Result<Option<ReceiptOutcome>>>>,
        nonce_results: Mutex<VecDeque<Result<u64>>>,
        fee_results: Mutex<VecDeque<Result<FeeEstimate>>>,
        pending_tx_fees_results: Mutex<VecDeque<Result<Option<PendingTxFees>>>>,
        bump_submit_results: Mutex<VecDeque<Result<String>>>,
    }

    #[async_trait]
    impl EthGateway for MockEthGateway {
        async fn submit_payload(&self, _payload_bytes: &[u8]) -> Result<String> {
            self.submit_results
                .lock()
                .expect("poisoned")
                .pop_front()
                .unwrap_or_else(|| Err(anyhow!("unexpected submit call")))
        }

        async fn poll_receipt(&self, _tx_hash: &str) -> Result<Option<ReceiptOutcome>> {
            self.poll_results
                .lock()
                .expect("poisoned")
                .pop_front()
                .unwrap_or_else(|| Err(anyhow!("unexpected poll call")))
        }

        async fn get_next_nonce(&self) -> Result<u64> {
            self.nonce_results
                .lock()
                .expect("poisoned")
                .pop_front()
                .unwrap_or(Ok(0))
        }

        async fn get_current_fees(&self) -> Result<FeeEstimate> {
            self.fee_results
                .lock()
                .expect("poisoned")
                .pop_front()
                .unwrap_or(Ok(FeeEstimate {
                    base_fee_per_gas: 1_000_000_000,
                    max_priority_fee_per_gas: 100_000_000,
                }))
        }

        async fn get_pending_tx_fees(&self, _tx_hash: &str) -> Result<Option<PendingTxFees>> {
            self.pending_tx_fees_results
                .lock()
                .expect("poisoned")
                .pop_front()
                .unwrap_or(Ok(Some(PendingTxFees {
                    max_fee_per_gas: 1_000_000_000,
                    max_priority_fee_per_gas: 100_000_000,
                    max_fee_per_blob_gas: Some(1),
                })))
        }

        async fn submit_payload_with_fees(
            &self,
            _payload_bytes: &[u8],
            _nonce: u64,
            _fees: &FeeOverrides,
        ) -> Result<String> {
            self.bump_submit_results
                .lock()
                .expect("poisoned")
                .pop_front()
                .unwrap_or_else(|| Err(anyhow!("unexpected bump submit call")))
        }
    }

    fn mk_job(status: JobStatus) -> RelayJob {
        RelayJob {
            job_id: "job-1".to_string(),
            status,
            payload_bytes: vec![1, 2, 3],
            tx_final: "0xaa".to_string(),
            state_root_hash: "0xbb".to_string(),
            client_ref: None,
            attempt_count: 0,
            tx_hash: None,
            submitted_at: None,
            block_number: None,
            last_error: None,
            next_attempt_at: Some(now_ts()),
            nonce: None,
            bump_count: 0,
            created_at: now_ts(),
            updated_at: now_ts(),
        }
    }

    fn cfg() -> WorkerConfig {
        WorkerConfig {
            max_attempts: 8,
            retry_initial_secs: 1,
            retry_max_secs: 5,
            receipt_poll_secs: 1,
            receipt_timeout_secs: Some(3),
            idle_sleep_ms: 20,
            fee_bump_after_secs: None,
            fee_bump_multiplier_pct: 20,
            fee_bump_max: 5,
        }
    }

    fn test_urls() -> (String, String, String) {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let admin_url = std::env::var("TEST_RELAYER_DB_ADMIN")
            .unwrap_or_else(|_| "postgres://postgres@localhost:5432/postgres".to_string());
        let mut url = Url::parse(&admin_url).expect("valid admin url");
        let db_name = format!(
            "relayer_worker_test_{}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        url.set_path(&format!("/{db_name}"));
        (admin_url, url.to_string(), db_name)
    }

    async fn drop_db(admin_url: &str, db_name: &str) -> Result<()> {
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(admin_url)
            .await?;
        sqlx::query(
            "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = $1 AND pid <> pg_backend_pid()",
        )
        .bind(db_name)
        .execute(&pool)
        .await?;
        let escaped = db_name.replace('"', "\"\"");
        let stmt = format!("DROP DATABASE IF EXISTS \"{escaped}\"");
        pool.execute(stmt.as_str()).await?;
        Ok(())
    }

    async fn setup_db() -> Result<(Db, String, String)> {
        let (admin_url, db_url, db_name) = test_urls();
        drop_db(&admin_url, &db_name).await?;
        let db = Db::connect(&db_url).await?;
        Ok((db, admin_url, db_name))
    }

    #[tokio::test]
    #[ignore = "requires local postgres"]
    async fn retryable_submit_error_then_confirmed() -> Result<()> {
        let (db, admin_url, db_name) = setup_db().await?;
        let gateway = MockEthGateway::default();
        gateway
            .submit_results
            .lock()
            .expect("poisoned")
            .push_back(Err(anyhow!("rpc unavailable")));
        gateway
            .submit_results
            .lock()
            .expect("poisoned")
            .push_back(Ok(
                "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
            ));
        gateway
            .poll_results
            .lock()
            .expect("poisoned")
            .push_back(Ok(Some(ReceiptOutcome {
                success: true,
                block_number: Some(42),
            })));

        let job = mk_job(JobStatus::Queued);
        db.insert_job(&job).await?;
        let cfg = cfg();

        let job = db.get_job("job-1").await?.expect("job");
        send_queued_job(&db, &gateway, &cfg, job).await?;
        let first = db.get_job("job-1").await?.expect("job");
        assert_eq!(first.status, JobStatus::Queued);
        assert_eq!(first.attempt_count, 1);
        assert!(first.last_error.is_some());

        let job = db.get_job("job-1").await?.expect("job");
        send_queued_job(&db, &gateway, &cfg, job).await?;
        let second = db.get_job("job-1").await?.expect("job");
        assert_eq!(second.status, JobStatus::Submitted);
        assert!(second.tx_hash.is_some());
        assert!(second.submitted_at.is_some());

        let job = db.get_job("job-1").await?.expect("job");
        poll_submitted_job(&db, &gateway, &cfg, job).await?;
        let final_job = db.get_job("job-1").await?.expect("job");
        assert_eq!(final_job.status, JobStatus::Confirmed);
        assert_eq!(final_job.block_number, Some(42));

        drop(db);
        drop_db(&admin_url, &db_name).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local postgres"]
    async fn submitted_job_times_out() -> Result<()> {
        let (db, admin_url, db_name) = setup_db().await?;
        let gateway = MockEthGateway::default();

        let mut job = mk_job(JobStatus::Submitted);
        job.tx_hash =
            Some("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string());
        job.submitted_at = Some(now_ts() - 100);
        db.insert_job(&job).await?;

        let cfg = cfg();
        let job = db.get_job("job-1").await?.expect("job");
        poll_submitted_job(&db, &gateway, &cfg, job).await?;
        let timed_out = db.get_job("job-1").await?.expect("job");
        assert_eq!(timed_out.status, JobStatus::Failed);
        assert!(timed_out
            .last_error
            .expect("error")
            .contains("receipt timeout after"));

        drop(db);
        drop_db(&admin_url, &db_name).await?;
        Ok(())
    }
}
