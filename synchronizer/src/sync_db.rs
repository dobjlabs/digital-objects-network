use alloy::primitives::B256;
use anyhow::{anyhow, Context, Result};
use pod2::middleware::Hash;
use sqlx::{postgres::PgPoolOptions, types::Json, Executor, PgPool, Row};
use url::Url;

use crate::app_db::{db_bytes_to_hash, hash_to_db_bytes, AppHead};

/// Sync cursor exposed to API callers.
///
/// `last_processed_slot` is the canonical consensus progress marker.
/// `last_processed_block_number` is auxiliary execution-layer progress metadata.
#[derive(Debug, Clone, Copy)]
pub struct SyncProgress {
    pub last_processed_slot: u32,
    pub last_processed_block_number: Option<u32>,
}

/// Per-slot app-state delta persisted in Postgres so apply/rollback can be replayed idempotently.
#[derive(Debug, Clone)]
pub struct SlotJournal {
    pub slot: u32,
    pub old_head: AppHead,
    pub new_head: AppHead,
}

/// Startup recovery task derived from rows that are pending or not fully applied.
#[derive(Debug, Clone)]
pub struct PendingRecovery {
    pub slot: u32,
    pub block_number: Option<u32>,
    pub op: JournalOp,
    pub journal: SlotJournal,
}

/// Journal operation type for recovery and rollback staging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JournalOp {
    Apply,
    Rollback,
}

impl JournalOp {
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "apply" => Ok(JournalOp::Apply),
            "rollback" => Ok(JournalOp::Rollback),
            _ => Err(anyhow!("invalid journal op: {s}")),
        }
    }
}

#[derive(Clone)]
pub struct SyncDb {
    pool: PgPool,
}

impl SyncDb {
    /// Connect to Postgres, ensure database exists, and bootstrap schema/indexes.
    pub async fn connect(database_url: &str) -> Result<Self> {
        ensure_database_exists(database_url).await?;
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(database_url)
            .await
            .with_context(|| "Failed to connect to sync metadata Postgres")?;
        let store = Self { pool };
        store.bootstrap().await?;
        Ok(store)
    }

    /// Create schema objects used by the sync control plane.
    async fn bootstrap(&self) -> Result<()> {
        let statements = [
            r#"
            CREATE TABLE IF NOT EXISTS sync_cursor (
                id SMALLINT PRIMARY KEY CHECK (id = 1),
                last_processed_slot INTEGER NULL,
                last_processed_block_number INTEGER NULL,
                updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
            )
            "#,
            r#"
            CREATE TABLE IF NOT EXISTS canonical_slots (
                slot INTEGER PRIMARY KEY,
                block_root BYTEA NULL,
                parent_root BYTEA NULL,
                execution_block_number INTEGER NULL,
                current_gsr BYTEA NULL,
                is_empty BOOLEAN NOT NULL,
                status TEXT NOT NULL CHECK (status IN ('pending','applied')),
                updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
            )
            "#,
            r#"
            CREATE UNIQUE INDEX IF NOT EXISTS canonical_slots_block_root_uq
                ON canonical_slots(block_root)
                WHERE block_root IS NOT NULL
            "#,
            r#"
            CREATE TABLE IF NOT EXISTS slot_apply_journal (
                slot INTEGER PRIMARY KEY REFERENCES canonical_slots(slot) ON DELETE CASCADE,
                block_root BYTEA NULL,
                old_head JSONB NOT NULL,
                new_head JSONB NOT NULL,
                op TEXT NOT NULL DEFAULT 'apply',
                kv_applied BOOLEAN NOT NULL DEFAULT FALSE,
                created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
                kv_applied_at TIMESTAMPTZ NULL
            )
            "#,
            r#"
            CREATE INDEX IF NOT EXISTS canonical_slots_status_slot_idx
                ON canonical_slots(status, slot)
            "#,
            r#"
            CREATE INDEX IF NOT EXISTS canonical_slots_gsr_block_idx
                ON canonical_slots(status, execution_block_number)
                WHERE current_gsr IS NOT NULL AND execution_block_number IS NOT NULL
            "#,
            r#"
            CREATE INDEX IF NOT EXISTS slot_apply_journal_kv_applied_slot_idx
                ON slot_apply_journal(kv_applied, slot)
            "#,
        ];

        for stmt in statements {
            self.pool.execute(stmt).await?;
        }

        Ok(())
    }

    /// Ensure cursor row exists and return the next slot to process.
    ///
    /// On first run, starts from `initial_start` when provided, otherwise current head.
    pub async fn ensure_cursor_and_get_start_slot(
        &self,
        head_slot: u32,
        initial_start: Option<u32>,
    ) -> Result<u32> {
        let bootstrap_start_slot = initial_start.unwrap_or(head_slot);
        sqlx::query(
            r#"
            INSERT INTO sync_cursor (id, last_processed_slot, last_processed_block_number)
            VALUES (1, NULL, NULL)
            ON CONFLICT (id) DO NOTHING
            "#,
        )
        .execute(&self.pool)
        .await?;

        let row = sqlx::query("SELECT last_processed_slot FROM sync_cursor WHERE id = 1")
            .fetch_one(&self.pool)
            .await?;
        let stored_last: Option<i32> = row.get("last_processed_slot");
        Ok(stored_last
            .map(|slot| slot as u32 + 1)
            .unwrap_or(bootstrap_start_slot))
    }

    pub async fn last_processed_slot(&self) -> Result<Option<u32>> {
        let row = sqlx::query("SELECT last_processed_slot FROM sync_cursor WHERE id = 1")
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.and_then(|r| {
            r.get::<Option<i32>, _>("last_processed_slot")
                .map(|s| s as u32)
        }))
    }

    pub async fn last_progress(&self) -> Result<Option<SyncProgress>> {
        let row = sqlx::query(
            "SELECT last_processed_slot, last_processed_block_number FROM sync_cursor WHERE id = 1",
        )
        .fetch_optional(&self.pool)
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };

        let Some(slot) = row.get::<Option<i32>, _>("last_processed_slot") else {
            return Ok(None);
        };

        Ok(Some(SyncProgress {
            last_processed_slot: slot as u32,
            last_processed_block_number: row
                .get::<Option<i32>, _>("last_processed_block_number")
                .map(|v| v as u32),
        }))
    }

    pub async fn slot_root(&self, slot: u32) -> Result<Option<B256>> {
        let row = sqlx::query(
            "SELECT block_root FROM canonical_slots WHERE slot = $1 AND status = 'applied'",
        )
        .bind(slot as i32)
        .fetch_optional(&self.pool)
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };
        let root: Option<Vec<u8>> = row.get("block_root");
        match root {
            Some(bytes) => {
                let arr: [u8; 32] = bytes
                    .as_slice()
                    .try_into()
                    .map_err(|_| anyhow!("invalid block_root length"))?;
                Ok(Some(B256::from(arr)))
            }
            None => Ok(None),
        }
    }

    /// Stage canonical slot metadata and journal atomically as `pending`.
    ///
    /// RocksDB writes occur in a later step.
    pub async fn save_pending_slot(
        &self,
        slot: u32,
        block_root: Option<B256>,
        parent_root: Option<B256>,
        block_number: Option<u32>,
        current_gsr: Option<Hash>,
        is_empty: bool,
        journal: &SlotJournal,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            r#"
            INSERT INTO canonical_slots(slot, block_root, parent_root, execution_block_number, current_gsr, is_empty, status)
            VALUES ($1, $2, $3, $4, $5, $6, 'pending')
            ON CONFLICT (slot) DO UPDATE
            SET block_root = EXCLUDED.block_root,
                parent_root = EXCLUDED.parent_root,
                execution_block_number = EXCLUDED.execution_block_number,
                current_gsr = EXCLUDED.current_gsr,
                is_empty = EXCLUDED.is_empty,
                status = 'pending',
                updated_at = now()
            "#,
        )
        .bind(slot as i32)
        .bind(block_root.map(|v| v.as_slice().to_vec()))
        .bind(parent_root.map(|v| v.as_slice().to_vec()))
        .bind(block_number.map(|v| v as i32))
        .bind(current_gsr.map(hash_to_db_bytes))
        .bind(is_empty)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO slot_apply_journal(slot, block_root, old_head, new_head, op, kv_applied)
            VALUES ($1, $2, $3, $4, 'apply', false)
            ON CONFLICT (slot) DO UPDATE
            SET block_root = EXCLUDED.block_root,
                old_head = EXCLUDED.old_head,
                new_head = EXCLUDED.new_head,
                op = 'apply',
                kv_applied = false,
                kv_applied_at = NULL
            "#,
        )
        .bind(slot as i32)
        .bind(block_root.map(|v| v.as_slice().to_vec()))
        .bind(Json(journal.old_head))
        .bind(Json(journal.new_head))
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }

    /// Mark slot journal/metadata as applied and advance the single-row cursor atomically.
    pub async fn finalize_slot_applied(&self, slot: u32, block_number: Option<u32>) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            "UPDATE slot_apply_journal SET kv_applied = true, kv_applied_at = now() WHERE slot = $1",
        )
        .bind(slot as i32)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "UPDATE canonical_slots SET status = 'applied', execution_block_number = $2, updated_at = now() WHERE slot = $1",
        )
        .bind(slot as i32)
        .bind(block_number.map(|v| v as i32))
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO sync_cursor (id, last_processed_slot, last_processed_block_number)
            VALUES (1, $1, $2)
            ON CONFLICT (id) DO UPDATE
            SET last_processed_slot = EXCLUDED.last_processed_slot,
                last_processed_block_number = EXCLUDED.last_processed_block_number,
                updated_at = now()
            "#,
        )
        .bind(slot as i32)
        .bind(block_number.map(|v| v as i32))
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }

    /// Return all slots that need startup recovery.
    ///
    /// A slot is recoverable when metadata is still `pending` or journal has `kv_applied=false`.
    pub async fn pending_recoveries(&self) -> Result<Vec<PendingRecovery>> {
        let rows = sqlx::query(
            r#"
            SELECT c.slot,
                   c.execution_block_number,
                   j.op,
                   j.old_head,
                   j.new_head
            FROM canonical_slots c
            JOIN slot_apply_journal j ON j.slot = c.slot
            WHERE c.status = 'pending' OR j.kv_applied = false
            ORDER BY c.slot ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        let mut recoveries = Vec::with_capacity(rows.len());
        for row in rows {
            recoveries.push(PendingRecovery {
                slot: row.get::<i32, _>("slot") as u32,
                block_number: row
                    .get::<Option<i32>, _>("execution_block_number")
                    .map(|v| v as u32),
                op: JournalOp::from_str(row.get::<&str, _>("op"))?,
                journal: SlotJournal {
                    slot: row.get::<i32, _>("slot") as u32,
                    old_head: row.get::<Json<AppHead>, _>("old_head").0,
                    new_head: row.get::<Json<AppHead>, _>("new_head").0,
                },
            });
        }

        Ok(recoveries)
    }

    /// Stage rollback in Postgres and return affected journals for head rewind replay.
    pub async fn rollback_to_slot(&self, keep_slot: u32) -> Result<Vec<SlotJournal>> {
        let rows = sqlx::query(
            r#"
            SELECT j.slot, j.old_head, j.new_head
            FROM slot_apply_journal j
            WHERE j.slot > $1
            ORDER BY j.slot DESC
            "#,
        )
        .bind(keep_slot as i32)
        .fetch_all(&self.pool)
        .await?;

        let mut journals = Vec::with_capacity(rows.len());
        for row in rows {
            journals.push(SlotJournal {
                slot: row.get::<i32, _>("slot") as u32,
                old_head: row.get::<Json<AppHead>, _>("old_head").0,
                new_head: row.get::<Json<AppHead>, _>("new_head").0,
            });
        }

        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "UPDATE slot_apply_journal SET op = 'rollback', kv_applied = false, kv_applied_at = NULL WHERE slot > $1",
        )
        .bind(keep_slot as i32)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "UPDATE canonical_slots SET status = 'pending', updated_at = now() WHERE slot > $1",
        )
        .bind(keep_slot as i32)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO sync_cursor (id, last_processed_slot, last_processed_block_number)
            VALUES (
                1,
                $1,
                (SELECT execution_block_number FROM canonical_slots WHERE slot = $1)
            )
            ON CONFLICT (id) DO UPDATE
            SET last_processed_slot = EXCLUDED.last_processed_slot,
                last_processed_block_number = EXCLUDED.last_processed_block_number,
                updated_at = now()
            "#,
        )
        .bind(keep_slot as i32)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(journals)
    }

    /// Finalize rollback by deleting pending canonical rows for the provided slots.
    pub async fn complete_rollback(&self, slots: &[u32]) -> Result<()> {
        if slots.is_empty() {
            return Ok(());
        }

        let slot_i32s: Vec<i32> = slots.iter().map(|s| *s as i32).collect();
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            r#"
            DELETE FROM canonical_slots c
            WHERE c.slot = ANY($1)
              AND c.status = 'pending'
              AND EXISTS (
                SELECT 1
                FROM slot_apply_journal j
                WHERE j.slot = c.slot
                  AND j.op = 'rollback'
              )
            "#,
        )
        .bind(slot_i32s)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn recent_gsrs(&self, min_block_number: Option<u32>) -> Result<Vec<(Hash, i64)>> {
        let Some(min_block_number) = min_block_number else {
            return Ok(Vec::new());
        };

        let rows = sqlx::query(
            r#"
            SELECT current_gsr, execution_block_number
            FROM canonical_slots
            WHERE status = 'applied'
              AND current_gsr IS NOT NULL
              AND execution_block_number IS NOT NULL
              AND execution_block_number >= $1
            ORDER BY execution_block_number ASC, slot ASC
            "#,
        )
        .bind(min_block_number as i32)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|row| {
                let current_gsr: Vec<u8> = row.get("current_gsr");
                let block_number = row.get::<i32, _>("execution_block_number");
                Ok((db_bytes_to_hash(&current_gsr)?, i64::from(block_number)))
            })
            .collect()
    }
}

/// Ensure target Postgres database exists (local/dev convenience).
async fn ensure_database_exists(database_url: &str) -> Result<()> {
    let parsed = Url::parse(database_url).with_context(|| "Invalid SYNC_METADATA_DB_URL value")?;
    let db_name = parsed
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .filter(|segment| !segment.is_empty())
        .ok_or_else(|| anyhow!("SYNC_METADATA_DB_URL must include a database name"))?;

    let mut admin_url = parsed.clone();
    admin_url.set_path("/postgres");

    let admin_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(admin_url.as_str())
        .await
        .with_context(|| "Failed to connect to postgres admin database")?;

    let exists = sqlx::query_scalar::<_, i32>("SELECT 1 FROM pg_database WHERE datname = $1")
        .bind(db_name)
        .fetch_optional(&admin_pool)
        .await?
        .is_some();

    if !exists {
        let escaped = db_name.replace('"', "\"\"");
        let create_stmt = format!("CREATE DATABASE \"{escaped}\"");
        sqlx::query(&create_stmt).execute(&admin_pool).await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{app_db::AppDb, state_machine::StateMachine};
    use common::proof::MockBlobParser;
    use pod2::middleware::{hash_values, Value};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, OnceLock};
    use std::time::{SystemTime, UNIX_EPOCH};
    use tempfile::TempDir;
    use tokio::sync::Mutex;

    fn unique_hash(n: i64) -> Hash {
        hash_values(&[Value::from(n)])
    }

    fn unique_head(block_number: u32, marker: i64) -> AppHead {
        AppHead {
            transactions_root: unique_hash(marker),
            nullifiers_root: unique_hash(marker + 1),
            state_root_gsrs_root: unique_hash(marker + 2),
            gsr_history_root: unique_hash(marker + 3),
            current_gsr: Some(unique_hash(marker + 4)),
            current_block_number: Some(block_number),
            tx_count: block_number as u64,
            nullifier_count: block_number as u64 + 1,
            gsr_count: block_number as u64 + 2,
        }
    }

    fn test_urls() -> (String, String, String) {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let pid = std::process::id();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let ctr = COUNTER.fetch_add(1, Ordering::Relaxed);
        let db_name = format!("syncdb_test_{}_{}_{}", pid, nanos, ctr);
        let admin_url = std::env::var("TEST_PG_ADMIN_URL")
            .unwrap_or_else(|_| "postgres://postgres:postgres@127.0.0.1:5432/postgres".into());
        let base = admin_url.trim_end_matches("/postgres");
        let db_url = format!("{}/{}", base, db_name);
        (db_name, db_url, admin_url)
    }

    async fn fresh_sync_db() -> Result<(SyncDb, String, String)> {
        static DB_TEST_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();
        let _guard = DB_TEST_MUTEX.get_or_init(|| Mutex::new(())).lock().await;
        let (db_name, db_url, admin_url) = test_urls();
        let sync_db = SyncDb::connect(&db_url).await?;
        Ok((sync_db, db_name, admin_url))
    }

    async fn drop_db(db_name: &str, admin_url: &str) -> Result<()> {
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(admin_url)
            .await?;
        sqlx::query("SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = $1")
            .bind(db_name)
            .execute(&pool)
            .await?;
        let escaped = db_name.replace('"', "\"\"");
        sqlx::query(&format!("DROP DATABASE IF EXISTS \"{escaped}\""))
            .execute(&pool)
            .await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_pending_recovery_roundtrip() -> Result<()> {
        let (sync_db, db_name, admin_url) = fresh_sync_db().await?;
        let journal = SlotJournal {
            slot: 10,
            old_head: AppHead::empty(),
            new_head: unique_head(7, 100),
        };
        sync_db
            .save_pending_slot(
                10,
                None,
                None,
                Some(7),
                journal.new_head.current_gsr,
                false,
                &journal,
            )
            .await?;

        let pending = sync_db.pending_recoveries().await?;
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].journal.new_head, journal.new_head);

        drop_db(&db_name, &admin_url).await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_recent_gsrs_query() -> Result<()> {
        let (sync_db, db_name, admin_url) = fresh_sync_db().await?;
        let j1 = SlotJournal {
            slot: 1,
            old_head: AppHead::empty(),
            new_head: unique_head(5, 10),
        };
        sync_db
            .save_pending_slot(1, None, None, Some(5), j1.new_head.current_gsr, false, &j1)
            .await?;
        sync_db.finalize_slot_applied(1, Some(5)).await?;

        let j2 = SlotJournal {
            slot: 2,
            old_head: j1.new_head,
            new_head: unique_head(9, 20),
        };
        sync_db
            .save_pending_slot(2, None, None, Some(9), j2.new_head.current_gsr, false, &j2)
            .await?;
        sync_db.finalize_slot_applied(2, Some(9)).await?;

        let recent = sync_db.recent_gsrs(Some(6)).await?;
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].0, j2.new_head.current_gsr.unwrap());

        drop_db(&db_name, &admin_url).await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_rollback_staging_replays_old_head() -> Result<()> {
        let (sync_db, db_name, admin_url) = fresh_sync_db().await?;
        let dir = TempDir::new()?;
        let app_db = AppDb::connect(dir.path().to_str().unwrap())?;
        let state_machine = StateMachine::new(app_db, Arc::new(MockBlobParser))?;

        let j1 = SlotJournal {
            slot: 1,
            old_head: AppHead::empty(),
            new_head: unique_head(1, 100),
        };
        state_machine.apply_journal(&j1)?;
        sync_db
            .save_pending_slot(1, None, None, Some(1), j1.new_head.current_gsr, false, &j1)
            .await?;
        sync_db.finalize_slot_applied(1, Some(1)).await?;
        state_machine.apply_delta_to_memory(&crate::state_machine::SlotDelta {
            old_head: j1.old_head,
            new_head: j1.new_head,
        })?;

        let j2 = SlotJournal {
            slot: 2,
            old_head: j1.new_head,
            new_head: unique_head(2, 200),
        };
        state_machine.apply_journal(&j2)?;
        sync_db
            .save_pending_slot(2, None, None, Some(2), j2.new_head.current_gsr, false, &j2)
            .await?;
        sync_db.finalize_slot_applied(2, Some(2)).await?;

        let journals = sync_db.rollback_to_slot(1).await?;
        state_machine.rollback_journals(&journals)?;
        assert_eq!(state_machine.head_snapshot()?, j1.new_head);

        drop_db(&db_name, &admin_url).await?;
        Ok(())
    }
}
