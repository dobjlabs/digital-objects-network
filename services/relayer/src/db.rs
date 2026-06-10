use anyhow::{anyhow, Context, Result};
use payload::{db_bytes_to_hash, hash_to_db_bytes};
use pod2::middleware::Hash;
use sqlx::{postgres::PgPoolOptions, Executor, PgPool, Row};

use crate::model::{JobStatus, RelayJob};

/// Result of inserting by idempotency key (`tx_final`).
pub enum InsertJobResult {
    /// A new row was inserted.
    Inserted,
    /// A row with this `tx_final` already existed.
    Existing(Box<RelayJob>),
}

/// Postgres-backed queue/state store for relay jobs.
pub struct Db {
    pool: PgPool,
}

impl Db {
    /// Connect to an existing Postgres database and bootstrap schema/indexes.
    ///
    /// The database itself must already exist — provisioning is an explicit
    /// operator step so the app role does not need the `CREATEDB` privilege.
    pub async fn connect(database_url: &str) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(database_url)
            .await
            .with_context(|| format!("connect relayer postgres at {database_url}"))?;

        let db = Self { pool };
        db.bootstrap().await?;
        Ok(db)
    }

    /// Create queue table/indexes used by API idempotency and worker scheduling.
    async fn bootstrap(&self) -> Result<()> {
        let statements = [
            r#"
            CREATE TABLE IF NOT EXISTS relay_jobs (
                job_id TEXT PRIMARY KEY,
                status TEXT NOT NULL CHECK (status IN ('queued','sending','submitted','confirmed','failed')),
                payload_bytes BYTEA NOT NULL,
                tx_final BYTEA NOT NULL UNIQUE,
                state_root BYTEA NOT NULL,
                client_ref TEXT NULL,
                attempt_count INTEGER NOT NULL,
                tx_hash TEXT NULL,
                submitted_at BIGINT NULL,
                block_number BIGINT NULL,
                last_error TEXT NULL,
                next_attempt_at BIGINT NULL,
                nonce BIGINT NULL,
                bump_count INTEGER NOT NULL DEFAULT 0,
                prev_tx_hashes TEXT[] NOT NULL DEFAULT '{}',
                created_at BIGINT NOT NULL,
                updated_at BIGINT NOT NULL
            )
            "#,
            r#"
            CREATE INDEX IF NOT EXISTS relay_jobs_status_due_created_idx
                ON relay_jobs(status, next_attempt_at, created_at)
                WHERE status IN ('queued', 'sending', 'submitted')
            "#,
            r#"
            CREATE INDEX IF NOT EXISTS relay_jobs_due_created_idx
                ON relay_jobs(next_attempt_at, created_at)
                WHERE status IN ('queued', 'sending', 'submitted')
            "#,
        ];

        for stmt in statements {
            self.pool.execute(stmt).await?;
        }

        Ok(())
    }

    /// Insert a new job, or return the existing row if `tx_final` already exists.
    pub async fn insert_job(&self, job: &RelayJob) -> Result<InsertJobResult> {
        let inserted = sqlx::query(
            r#"
            INSERT INTO relay_jobs (
                job_id,
                status,
                payload_bytes,
                tx_final,
                state_root,
                client_ref,
                attempt_count,
                tx_hash,
                submitted_at,
                block_number,
                last_error,
                next_attempt_at,
                nonce,
                bump_count,
                prev_tx_hashes,
                created_at,
                updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17)
            ON CONFLICT (tx_final) DO NOTHING
            RETURNING job_id
            "#,
        )
        .bind(&job.job_id)
        .bind(job.status.as_str())
        .bind(&job.payload_bytes)
        .bind(hash_to_db_bytes(job.tx_final))
        .bind(hash_to_db_bytes(job.state_root))
        .bind(job.client_ref.clone())
        .bind(job.attempt_count as i32)
        .bind(job.tx_hash.clone())
        .bind(job.submitted_at)
        .bind(job.block_number.map(|value| value as i64))
        .bind(job.last_error.clone())
        .bind(job.next_attempt_at)
        .bind(job.nonce)
        .bind(job.bump_count)
        .bind(&job.prev_tx_hashes)
        .bind(job.created_at)
        .bind(job.updated_at)
        .fetch_optional(&self.pool)
        .await?;

        if inserted.is_some() {
            return Ok(InsertJobResult::Inserted);
        }

        if let Some(existing) = self.get_job_by_tx_final(&job.tx_final).await? {
            Ok(InsertJobResult::Existing(Box::new(existing)))
        } else {
            Err(anyhow!(
                "idempotent insert conflict but existing job not found for tx_final={:#}",
                job.tx_final
            ))
        }
    }

    /// Persist the current in-memory state of a known job row.
    pub async fn put_job(&self, job: &RelayJob) -> Result<()> {
        let rows_affected = sqlx::query(
            r#"
            UPDATE relay_jobs
            SET status = $2,
                payload_bytes = $3,
                tx_final = $4,
                state_root = $5,
                client_ref = $6,
                attempt_count = $7,
                tx_hash = $8,
                submitted_at = $9,
                block_number = $10,
                last_error = $11,
                next_attempt_at = $12,
                nonce = $13,
                bump_count = $14,
                prev_tx_hashes = $15,
                created_at = $16,
                updated_at = $17
            WHERE job_id = $1
            "#,
        )
        .bind(&job.job_id)
        .bind(job.status.as_str())
        .bind(&job.payload_bytes)
        .bind(hash_to_db_bytes(job.tx_final))
        .bind(hash_to_db_bytes(job.state_root))
        .bind(job.client_ref.clone())
        .bind(job.attempt_count as i32)
        .bind(job.tx_hash.clone())
        .bind(job.submitted_at)
        .bind(job.block_number.map(|value| value as i64))
        .bind(job.last_error.clone())
        .bind(job.next_attempt_at)
        .bind(job.nonce)
        .bind(job.bump_count)
        .bind(&job.prev_tx_hashes)
        .bind(job.created_at)
        .bind(job.updated_at)
        .execute(&self.pool)
        .await?
        .rows_affected();

        if rows_affected == 0 {
            return Err(anyhow!("job not found: {}", job.job_id));
        }

        Ok(())
    }

    /// Lookup by API-facing `job_id`.
    pub async fn get_job(&self, job_id: &str) -> Result<Option<RelayJob>> {
        let row = sqlx::query(
            r#"
            SELECT job_id,
                   status,
                   payload_bytes,
                   tx_final,
                   state_root,
                   client_ref,
                   attempt_count,
                   tx_hash,
                   submitted_at,
                   block_number,
                   last_error,
                   next_attempt_at,
                   nonce,
                   bump_count,
                   prev_tx_hashes,
                   created_at,
                   updated_at
            FROM relay_jobs
            WHERE job_id = $1
            "#,
        )
        .bind(job_id)
        .fetch_optional(&self.pool)
        .await?;

        row.map(row_to_job).transpose()
    }

    /// Lookup by idempotency key.
    pub async fn get_job_by_tx_final(&self, tx_final: &Hash) -> Result<Option<RelayJob>> {
        let row = sqlx::query(
            r#"
            SELECT job_id,
                   status,
                   payload_bytes,
                   tx_final,
                   state_root,
                   client_ref,
                   attempt_count,
                   tx_hash,
                   submitted_at,
                   block_number,
                   last_error,
                   next_attempt_at,
                   nonce,
                   bump_count,
                   prev_tx_hashes,
                   created_at,
                   updated_at
            FROM relay_jobs
            WHERE tx_final = $1
            "#,
        )
        .bind(hash_to_db_bytes(*tx_final))
        .fetch_optional(&self.pool)
        .await?;

        row.map(row_to_job).transpose()
    }

    /// Resolve the current Ethereum tx hash for each given `tx_final` that
    /// exists and has been broadcast. Unknown or not-yet-broadcast commitments
    /// are omitted from the result.
    pub async fn tx_hashes_by_tx_finals(&self, tx_finals: &[Hash]) -> Result<Vec<(Hash, String)>> {
        let keys: Vec<Vec<u8>> = tx_finals.iter().map(|h| hash_to_db_bytes(*h)).collect();
        let rows = sqlx::query(
            r#"
            SELECT tx_final, tx_hash
            FROM relay_jobs
            WHERE tx_final = ANY($1::bytea[]) AND tx_hash IS NOT NULL
            "#,
        )
        .bind(keys)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|row| {
                let tx_final = db_bytes_to_hash(&row.get::<Vec<u8>, _>("tx_final"))?;
                let tx_hash: String = row.get("tx_hash");
                Ok((tx_final, tx_hash))
            })
            .collect()
    }

    /// Requeue in-flight states that can be left behind on crash/restart.
    pub async fn recover_inflight_jobs(&self, now: i64) -> Result<usize> {
        let result = sqlx::query(
            r#"
            UPDATE relay_jobs
            SET status = 'queued',
                next_attempt_at = $1,
                updated_at = $1
            WHERE status = 'sending'
               OR (status = 'submitted' AND tx_hash IS NULL)
            "#,
        )
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() as usize)
    }

    /// Pick the next due non-terminal job by schedule time.
    pub async fn next_due_job(&self, now: i64) -> Result<Option<RelayJob>> {
        let row = sqlx::query(
            r#"
            SELECT job_id,
                   status,
                   payload_bytes,
                   tx_final,
                   state_root,
                   client_ref,
                   attempt_count,
                   tx_hash,
                   submitted_at,
                   block_number,
                   last_error,
                   next_attempt_at,
                   nonce,
                   bump_count,
                   prev_tx_hashes,
                   created_at,
                   updated_at
            FROM relay_jobs
            WHERE status IN ('queued', 'sending', 'submitted')
              AND (next_attempt_at IS NULL OR next_attempt_at <= $1)
            ORDER BY COALESCE(next_attempt_at, created_at), created_at
            LIMIT 1
            "#,
        )
        .bind(now)
        .fetch_optional(&self.pool)
        .await?;

        row.map(row_to_job).transpose()
    }
}

/// Decode a SQL row into `RelayJob` with explicit integer range checks.
fn row_to_job(row: sqlx::postgres::PgRow) -> Result<RelayJob> {
    let status_raw: &str = row.get("status");
    // wire-types' `from_db_str` returns `Result<_, String>` to stay
    // anyhow-free; map into `anyhow::Error` here at the server-side boundary.
    let status = JobStatus::from_db_str(status_raw).map_err(anyhow::Error::msg)?;

    let attempt_count_i32: i32 = row.get("attempt_count");
    let attempt_count: u32 = attempt_count_i32
        .try_into()
        .map_err(|_| anyhow!("invalid attempt_count: {attempt_count_i32}"))?;

    let block_number: Option<i64> = row.get("block_number");
    let block_number = block_number
        .map(|value| u64::try_from(value).map_err(|_| anyhow!("invalid block_number: {value}")))
        .transpose()?;

    Ok(RelayJob {
        job_id: row.get("job_id"),
        status,
        payload_bytes: row.get("payload_bytes"),
        tx_final: db_bytes_to_hash(&row.get::<Vec<u8>, _>("tx_final"))?,
        state_root: db_bytes_to_hash(&row.get::<Vec<u8>, _>("state_root"))?,
        client_ref: row.get("client_ref"),
        attempt_count,
        tx_hash: row.get("tx_hash"),
        submitted_at: row.get("submitted_at"),
        block_number,
        last_error: row.get("last_error"),
        next_attempt_at: row.get("next_attempt_at"),
        nonce: row.get("nonce"),
        bump_count: row.get("bump_count"),
        prev_tx_hashes: row.get("prev_tx_hashes"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{JobStatus, RelayJob};
    use pod2::middleware::RawValue;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::OnceLock;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tokio::sync::Mutex;
    use url::Url;

    fn now() -> i64 {
        1_700_000_000
    }

    fn th(byte: u8) -> Hash {
        Hash::from(RawValue::from(i64::from(byte)))
    }

    fn mk_job(id: &str, tx_final: Hash, status: JobStatus, next: Option<i64>) -> RelayJob {
        RelayJob {
            job_id: id.to_string(),
            status,
            payload_bytes: vec![1, 2, 3],
            tx_final,
            state_root: Hash::default(),
            client_ref: None,
            attempt_count: 0,
            tx_hash: None,
            submitted_at: None,
            block_number: None,
            last_error: None,
            next_attempt_at: next,
            nonce: None,
            bump_count: 0,
            prev_tx_hashes: Vec::new(),
            created_at: now(),
            updated_at: now(),
        }
    }

    fn test_urls() -> (String, String, String) {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let admin_url = std::env::var("TEST_RELAYER_DB_ADMIN")
            .unwrap_or_else(|_| "postgres://postgres@localhost:5432/postgres".to_string());
        let mut url = Url::parse(&admin_url).expect("valid admin url");
        let db_name = format!(
            "relayer_test_{}_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        url.set_path(&format!("/{db_name}"));
        (admin_url, url.to_string(), db_name)
    }

    fn test_db_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
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

    async fn create_db(admin_url: &str, db_name: &str) -> Result<()> {
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(admin_url)
            .await?;
        let escaped = db_name.replace('"', "\"\"");
        pool.execute(format!("CREATE DATABASE \"{escaped}\"").as_str())
            .await?;
        Ok(())
    }

    async fn setup_db() -> Result<(Db, String, String)> {
        let (admin_url, db_url, db_name) = test_urls();
        drop_db(&admin_url, &db_name).await?;
        create_db(&admin_url, &db_name).await?;
        let db = Db::connect(&db_url).await?;
        Ok((db, admin_url, db_name))
    }

    #[tokio::test]
    #[ignore = "requires local postgres"]
    async fn idempotent_insert_by_tx_final() -> Result<()> {
        let _guard = test_db_lock().lock().await;
        let (db, admin_url, db_name) = setup_db().await?;

        let job = mk_job("job-1", th(0xaa), JobStatus::Queued, Some(now()));
        let inserted = db.insert_job(&job).await?;
        assert!(matches!(inserted, InsertJobResult::Inserted));

        let second = mk_job("job-2", th(0xaa), JobStatus::Queued, Some(now()));
        let existing = db.insert_job(&second).await?;
        match existing {
            InsertJobResult::Existing(found) => assert_eq!(found.job_id, "job-1"),
            InsertJobResult::Inserted => panic!("expected existing"),
        }

        drop(db);
        drop_db(&admin_url, &db_name).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local postgres"]
    async fn recover_sending_to_queued() -> Result<()> {
        let _guard = test_db_lock().lock().await;
        let (db, admin_url, db_name) = setup_db().await?;

        let mut sending = mk_job("job-1", th(0xaa), JobStatus::Sending, None);
        sending.next_attempt_at = None;
        db.insert_job(&sending).await?;

        let updated = db.recover_inflight_jobs(now()).await?;
        assert_eq!(updated, 1);
        let got = db.get_job("job-1").await?.expect("job");
        assert_eq!(got.status, JobStatus::Queued);
        assert_eq!(got.next_attempt_at, Some(now()));

        drop(db);
        drop_db(&admin_url, &db_name).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local postgres"]
    async fn recover_submitted_without_hash_to_queued() -> Result<()> {
        let _guard = test_db_lock().lock().await;
        let (db, admin_url, db_name) = setup_db().await?;

        let submitted = mk_job("job-1", th(0xaa), JobStatus::Submitted, None);
        db.insert_job(&submitted).await?;

        let updated = db.recover_inflight_jobs(now()).await?;
        assert_eq!(updated, 1);
        let got = db.get_job("job-1").await?.expect("job");
        assert_eq!(got.status, JobStatus::Queued);
        assert_eq!(got.next_attempt_at, Some(now()));

        drop(db);
        drop_db(&admin_url, &db_name).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local postgres"]
    async fn picks_next_due_job() -> Result<()> {
        let _guard = test_db_lock().lock().await;
        let (db, admin_url, db_name) = setup_db().await?;

        let a = mk_job("job-a", th(0xaa), JobStatus::Queued, Some(now() + 5));
        let b = mk_job("job-b", th(0xbb), JobStatus::Queued, Some(now()));
        db.insert_job(&a).await?;
        db.insert_job(&b).await?;

        let next = db.next_due_job(now()).await?.expect("due job");
        assert_eq!(next.job_id, "job-b");

        drop(db);
        drop_db(&admin_url, &db_name).await?;
        Ok(())
    }
}
