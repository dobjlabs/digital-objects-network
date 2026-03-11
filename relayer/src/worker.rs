use std::{sync::Arc, time::Duration};

use anyhow::{Result, anyhow};
use tokio::sync::watch;
use tracing::{info, warn};

use crate::{
    db::Db,
    eth::{EthGateway, ReceiptOutcome},
    model::{JobStatus, RelayJob},
};

#[derive(Clone)]
pub struct WorkerConfig {
    pub max_attempts: u32,
    pub retry_initial_secs: u64,
    pub retry_max_secs: u64,
    pub receipt_poll_secs: u64,
    pub receipt_timeout_secs: Option<u64>,
    pub idle_sleep_ms: u64,
}

pub async fn run_worker(
    db: Arc<Db>,
    eth_client: Arc<dyn EthGateway>,
    cfg: WorkerConfig,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<()> {
    info!(
        max_attempts = cfg.max_attempts,
        retry_initial_secs = cfg.retry_initial_secs,
        retry_max_secs = cfg.retry_max_secs,
        receipt_poll_secs = cfg.receipt_poll_secs,
        receipt_timeout_secs = ?cfg.receipt_timeout_secs,
        idle_sleep_ms = cfg.idle_sleep_ms,
        "Relayer worker started"
    );

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

async fn send_queued_job(
    db: &Db,
    eth_client: &dyn EthGateway,
    cfg: &WorkerConfig,
    mut job: RelayJob,
) -> Result<()> {
    let now = now_ts();
    job.status = JobStatus::Sending;
    job.updated_at = now;
    db.put_job(&job).await?;

    job.attempt_count = job.attempt_count.saturating_add(1);
    job.updated_at = now;
    db.put_job(&job).await?;

    info!(
        job_id = %job.job_id,
        attempt = job.attempt_count,
        payload_bytes = job.payload_bytes.len(),
        tx_final = %job.tx_final,
        state_root_hash = %job.state_root_hash,
        "Submitting relay payload to Ethereum"
    );

    match eth_client.submit_payload(&job.payload_bytes).await {
        Ok(tx_hash) => {
            job.status = JobStatus::Submitted;
            job.tx_hash = Some(tx_hash);
            job.submitted_at = Some(now);
            job.next_attempt_at = Some(now + cfg.receipt_poll_secs as i64);
            job.last_error = None;
            job.updated_at = now;
            db.put_job(&job).await?;
            info!(
                job_id = %job.job_id,
                tx_hash = ?job.tx_hash,
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
            fail_job(
                db,
                &mut job,
                now,
                format!("receipt timeout after {}s", timeout_secs),
            )
            .await?;
            return Ok(());
        }
    }

    job.attempt_count = job.attempt_count.saturating_add(1);
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
                fail_job(db, &mut job, now, err.to_string()).await?;
                return Ok(());
            }
            schedule_retry_or_fail(db, cfg, job, now, err).await?;
            Ok(())
        }
    }
}

async fn fail_job(db: &Db, job: &mut RelayJob, now: i64, reason: String) -> Result<()> {
    fail_job_with_block(db, job, now, job.block_number, reason).await
}

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

fn now_ts() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, sync::Mutex};

    use async_trait::async_trait;
    use sqlx::{Executor, postgres::PgPoolOptions};
    use url::Url;

    use super::*;

    #[derive(Default)]
    struct MockEthGateway {
        submit_results: Mutex<VecDeque<Result<String>>>,
        poll_results: Mutex<VecDeque<Result<Option<ReceiptOutcome>>>>,
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
        db.insert_job_idempotent(&job).await?;
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
        db.insert_job_idempotent(&job).await?;

        let cfg = cfg();
        let job = db.get_job("job-1").await?.expect("job");
        poll_submitted_job(&db, &gateway, &cfg, job).await?;
        let timed_out = db.get_job("job-1").await?.expect("job");
        assert_eq!(timed_out.status, JobStatus::Failed);
        assert!(
            timed_out
                .last_error
                .expect("error")
                .contains("receipt timeout after")
        );

        drop(db);
        drop_db(&admin_url, &db_name).await?;
        Ok(())
    }
}
