use alloy::primitives::B256;
use anyhow::{anyhow, Context, Result};
use pod2::middleware::Hash;
use sqlx::{postgres::PgPoolOptions, Executor, PgPool, Row};
use url::Url;

use crate::app_db::{db_bytes_to_hash, hash_to_db_bytes};

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
    pub tx_hashes: Vec<Hash>,
    pub nullifiers: Vec<Hash>,
    pub gsr_block_numbers: Vec<u32>,
    pub gsr_hashes: Vec<Hash>,
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
    ///
    /// Creates:
    /// 1. `sync_cursor` for single-row progress tracking
    /// 2. `canonical_slots` for canonical slot metadata + status
    /// 3. `slot_apply_journal` for per-slot apply/rollback replay
    /// 4. supporting indexes for recovery and lookup paths
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
                tx_hashes BYTEA[] NOT NULL DEFAULT '{}'::bytea[],
                nullifiers BYTEA[] NOT NULL DEFAULT '{}'::bytea[],
                gsr_block_numbers INTEGER[] NOT NULL DEFAULT '{}'::integer[],
                gsr_hashes BYTEA[] NOT NULL DEFAULT '{}'::bytea[],
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
        is_empty: bool,
        journal: &SlotJournal,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            r#"
            INSERT INTO canonical_slots(slot, block_root, parent_root, execution_block_number, is_empty, status)
            VALUES ($1, $2, $3, $4, $5, 'pending')
            ON CONFLICT (slot) DO UPDATE
            SET block_root = EXCLUDED.block_root,
                parent_root = EXCLUDED.parent_root,
                execution_block_number = EXCLUDED.execution_block_number,
                is_empty = EXCLUDED.is_empty,
                status = 'pending',
                updated_at = now()
            "#,
        )
        .bind(slot as i32)
        .bind(block_root.map(|v| v.as_slice().to_vec()))
        .bind(parent_root.map(|v| v.as_slice().to_vec()))
        .bind(block_number.map(|v| v as i32))
        .bind(is_empty)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO slot_apply_journal(slot, block_root, tx_hashes, nullifiers, gsr_block_numbers, gsr_hashes, op, kv_applied)
            VALUES ($1, $2, $3, $4, $5, $6, 'apply', false)
            ON CONFLICT (slot) DO UPDATE
            SET block_root = EXCLUDED.block_root,
                tx_hashes = EXCLUDED.tx_hashes,
                nullifiers = EXCLUDED.nullifiers,
                gsr_block_numbers = EXCLUDED.gsr_block_numbers,
                gsr_hashes = EXCLUDED.gsr_hashes,
                op = 'apply',
                kv_applied = false,
                kv_applied_at = NULL
            "#,
        )
        .bind(slot as i32)
        .bind(block_root.map(|v| v.as_slice().to_vec()))
        .bind(journal.tx_hashes.iter().map(|h| hash_to_db_bytes(*h)).collect::<Vec<_>>())
        .bind(
            journal
                .nullifiers
                .iter()
                .map(|h| hash_to_db_bytes(*h))
                .collect::<Vec<_>>(),
        )
        .bind(
            journal
                .gsr_block_numbers
                .iter()
                .map(|v| *v as i32)
                .collect::<Vec<_>>(),
        )
        .bind(
            journal
                .gsr_hashes
                .iter()
                .map(|h| hash_to_db_bytes(*h))
                .collect::<Vec<_>>(),
        )
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }

    /// Mark slot journal/metadata as applied and advance the single-row cursor atomically.
    pub async fn finalize_slot_applied(&self, slot: u32, block_number: Option<u32>) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        sqlx::query("UPDATE slot_apply_journal SET kv_applied = true, kv_applied_at = now() WHERE slot = $1")
            .bind(slot as i32)
            .execute(&mut *tx)
            .await?;

        sqlx::query("UPDATE canonical_slots SET status = 'applied', execution_block_number = $2, updated_at = now() WHERE slot = $1")
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
                   j.tx_hashes,
                   j.nullifiers,
                   j.gsr_block_numbers,
                   j.gsr_hashes
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
            let tx_hash_bytes: Vec<Vec<u8>> = row.get("tx_hashes");
            let nullifier_bytes: Vec<Vec<u8>> = row.get("nullifiers");
            let gsr_hash_bytes: Vec<Vec<u8>> = row.get("gsr_hashes");
            let gsr_block_numbers: Vec<i32> = row.get("gsr_block_numbers");
            let op = JournalOp::from_str(row.get::<&str, _>("op"))?;
            recoveries.push(PendingRecovery {
                slot: row.get::<i32, _>("slot") as u32,
                block_number: row
                    .get::<Option<i32>, _>("execution_block_number")
                    .map(|v| v as u32),
                op,
                journal: SlotJournal {
                    slot: row.get::<i32, _>("slot") as u32,
                    tx_hashes: tx_hash_bytes
                        .iter()
                        .map(|b| db_bytes_to_hash(b))
                        .collect::<Result<Vec<_>>>()?,
                    nullifiers: nullifier_bytes
                        .iter()
                        .map(|b| db_bytes_to_hash(b))
                        .collect::<Result<Vec<_>>>()?,
                    gsr_block_numbers: gsr_block_numbers.iter().map(|v| *v as u32).collect(),
                    gsr_hashes: gsr_hash_bytes
                        .iter()
                        .map(|b| db_bytes_to_hash(b))
                        .collect::<Result<Vec<_>>>()?,
                },
            });
        }

        Ok(recoveries)
    }

    /// Stage rollback in Postgres and return affected journals for RocksDB delete replay.
    ///
    /// Two-phase rollback:
    /// 1. Mark affected slots/journals as rollback-pending and rewind cursor in Postgres.
    /// 2. Caller replays journal deletes in RocksDB.
    /// 3. Caller invokes `complete_rollback` to finalize and delete pending rows.
    pub async fn rollback_to_slot(&self, keep_slot: Option<u32>) -> Result<Vec<SlotJournal>> {
        let rows = if let Some(keep_slot) = keep_slot {
            sqlx::query(
                r#"
                SELECT j.slot, j.tx_hashes, j.nullifiers, j.gsr_block_numbers, j.gsr_hashes
                FROM slot_apply_journal j
                WHERE j.slot > $1
                ORDER BY j.slot DESC
                "#,
            )
            .bind(keep_slot as i32)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                r#"
                SELECT j.slot, j.tx_hashes, j.nullifiers, j.gsr_block_numbers, j.gsr_hashes
                FROM slot_apply_journal j
                ORDER BY j.slot DESC
                "#,
            )
            .fetch_all(&self.pool)
            .await?
        };

        let mut journals = Vec::with_capacity(rows.len());
        for row in rows {
            let tx_hash_bytes: Vec<Vec<u8>> = row.get("tx_hashes");
            let nullifier_bytes: Vec<Vec<u8>> = row.get("nullifiers");
            let gsr_hash_bytes: Vec<Vec<u8>> = row.get("gsr_hashes");
            let gsr_block_numbers: Vec<i32> = row.get("gsr_block_numbers");
            journals.push(SlotJournal {
                slot: row.get::<i32, _>("slot") as u32,
                tx_hashes: tx_hash_bytes
                    .iter()
                    .map(|b| db_bytes_to_hash(b))
                    .collect::<Result<Vec<_>>>()?,
                nullifiers: nullifier_bytes
                    .iter()
                    .map(|b| db_bytes_to_hash(b))
                    .collect::<Result<Vec<_>>>()?,
                gsr_block_numbers: gsr_block_numbers.iter().map(|v| *v as u32).collect(),
                gsr_hashes: gsr_hash_bytes
                    .iter()
                    .map(|b| db_bytes_to_hash(b))
                    .collect::<Result<Vec<_>>>()?,
            });
        }

        let mut tx = self.pool.begin().await?;
        if let Some(keep_slot) = keep_slot {
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
        } else {
            sqlx::query(
                "UPDATE slot_apply_journal SET op = 'rollback', kv_applied = false, kv_applied_at = NULL",
            )
            .execute(&mut *tx)
            .await?;

            sqlx::query("UPDATE canonical_slots SET status = 'pending', updated_at = now()")
                .execute(&mut *tx)
                .await?;

            sqlx::query(
                r#"
                INSERT INTO sync_cursor (id, last_processed_slot, last_processed_block_number)
                VALUES (1, NULL, NULL)
                ON CONFLICT (id) DO UPDATE
                SET last_processed_slot = NULL,
                    last_processed_block_number = NULL,
                    updated_at = now()
                "#,
            )
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(journals)
    }

    /// Finalize rollback by deleting pending canonical rows for the provided slots.
    ///
    /// Steps:
    /// 1. Delete rollback-pending rows from `canonical_slots` for provided slots.
    /// 2. Let FK cascade delete matching `slot_apply_journal` rows.
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
}

/// Ensure target Postgres database exists (local/dev convenience).
async fn ensure_database_exists(database_url: &str) -> Result<()> {
    let parsed = Url::parse(database_url).with_context(|| "Invalid SYNC_METADATA_DB URL")?;
    let db_name = parsed
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .filter(|segment| !segment.is_empty())
        .ok_or_else(|| anyhow!("SYNC_METADATA_DB must include a database name"))?;

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
    use crate::app_db::AppDb;
    use crate::proof::MockBlobParser;
    use crate::state_machine::StateMachine;
    use pod2::middleware::{hash_values, Value};
    use sqlx::Executor;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::{SystemTime, UNIX_EPOCH};
    use tempfile::TempDir;

    fn unique_hash(n: i64) -> Hash {
        hash_values(&[Value::from(n)])
    }

    fn test_urls() -> (String, String, String) {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let admin_url = std::env::var("TEST_SYNC_METADATA_DB_ADMIN")
            .unwrap_or_else(|_| "postgres://postgres@localhost:5432/postgres".to_string());
        let mut url = Url::parse(&admin_url).expect("valid admin url");
        let db_name = format!(
            "sync_test_{}_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        url.set_path(&format!("/{}", db_name));
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

    #[tokio::test]
    #[ignore = "requires local postgres"]
    async fn test_recover_pending_replays_journal_and_finalizes() -> Result<()> {
        let _guard = test_db_lock().lock().expect("lock");
        let (admin_url, db_url, db_name) = test_urls();
        drop_db(&admin_url, &db_name).await?;
        let sync_db = SyncDb::connect(&db_url).await?;

        let dir = TempDir::new().unwrap();
        let app_db = AppDb::connect(dir.path().to_str().unwrap())?;
        let state_machine = StateMachine::new(app_db, Arc::new(MockBlobParser))?;

        let slot = 100;
        let block_number = 4242;
        let tx_hash = unique_hash(1);
        let nullifier = unique_hash(2);
        let gsr_hash = unique_hash(3);
        let journal = SlotJournal {
            slot,
            tx_hashes: vec![tx_hash],
            nullifiers: vec![nullifier],
            gsr_block_numbers: vec![block_number],
            gsr_hashes: vec![gsr_hash],
        };
        let root = B256::from([7u8; 32]);
        let parent = B256::from([6u8; 32]);
        sync_db
            .save_pending_slot(
                slot,
                Some(root),
                Some(parent),
                Some(block_number),
                false,
                &journal,
            )
            .await?;

        let pending = sync_db.pending_recoveries().await?;
        assert_eq!(pending.len(), 1);
        let recovery = &pending[0];
        state_machine.apply_journal(&recovery.journal)?;
        sync_db
            .finalize_slot_applied(recovery.slot, recovery.block_number)
            .await?;
        state_machine.reload_from_db()?;

        assert!(sync_db.pending_recoveries().await?.is_empty());
        let progress = sync_db.last_progress().await?.expect("progress");
        assert_eq!(progress.last_processed_slot, slot);
        assert_eq!(progress.last_processed_block_number, Some(block_number));

        let (txs, nullifiers, gsrs) = state_machine.state_snapshot()?;
        assert!(txs.contains(&tx_hash));
        assert!(nullifiers.contains(&nullifier));
        assert!(gsrs.contains(&gsr_hash));

        drop(sync_db);
        drop_db(&admin_url, &db_name).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local postgres"]
    async fn test_rollback_to_slot_rewinds_pg_and_kv() -> Result<()> {
        let _guard = test_db_lock().lock().expect("lock");
        let (admin_url, db_url, db_name) = test_urls();
        drop_db(&admin_url, &db_name).await?;
        let sync_db = SyncDb::connect(&db_url).await?;

        let dir = TempDir::new().unwrap();
        let app_db = AppDb::connect(dir.path().to_str().unwrap())?;
        let state_machine = StateMachine::new(app_db, Arc::new(MockBlobParser))?;

        let root1 = B256::from([1u8; 32]);
        let root2 = B256::from([2u8; 32]);
        let parent = B256::from([9u8; 32]);
        let j1 = SlotJournal {
            slot: 10,
            tx_hashes: vec![unique_hash(10)],
            nullifiers: vec![unique_hash(11)],
            gsr_block_numbers: vec![1000],
            gsr_hashes: vec![unique_hash(12)],
        };
        sync_db
            .save_pending_slot(10, Some(root1), Some(parent), Some(1000), false, &j1)
            .await?;
        state_machine.apply_journal(&j1)?;
        sync_db.finalize_slot_applied(10, Some(1000)).await?;

        let j2 = SlotJournal {
            slot: 11,
            tx_hashes: vec![unique_hash(20)],
            nullifiers: vec![unique_hash(21)],
            gsr_block_numbers: vec![1001],
            gsr_hashes: vec![unique_hash(22)],
        };
        sync_db
            .save_pending_slot(11, Some(root2), Some(root1), Some(1001), false, &j2)
            .await?;
        state_machine.apply_journal(&j2)?;
        sync_db.finalize_slot_applied(11, Some(1001)).await?;
        state_machine.reload_from_db()?;

        let journals = sync_db.rollback_to_slot(Some(10)).await?;
        state_machine.rollback_journals(&journals)?;

        let progress = sync_db.last_progress().await?.expect("progress");
        assert_eq!(progress.last_processed_slot, 10);
        assert_eq!(progress.last_processed_block_number, Some(1000));
        assert_eq!(sync_db.slot_root(10).await?, Some(root1));
        assert_eq!(sync_db.slot_root(11).await?, None);

        let (txs, nullifiers, gsrs) = state_machine.state_snapshot()?;
        assert!(txs.contains(&j1.tx_hashes[0]));
        assert!(!txs.contains(&j2.tx_hashes[0]));
        assert!(nullifiers.contains(&j1.nullifiers[0]));
        assert!(!nullifiers.contains(&j2.nullifiers[0]));
        assert!(gsrs.contains(&j1.gsr_hashes[0]));
        assert!(!gsrs.contains(&j2.gsr_hashes[0]));

        drop(sync_db);
        drop_db(&admin_url, &db_name).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local postgres"]
    async fn test_rollback_staging_survives_crash_and_recovers() -> Result<()> {
        let _guard = test_db_lock().lock().expect("lock");
        let (admin_url, db_url, db_name) = test_urls();
        drop_db(&admin_url, &db_name).await?;
        let sync_db = SyncDb::connect(&db_url).await?;

        let dir = TempDir::new().unwrap();
        let app_db = AppDb::connect(dir.path().to_str().unwrap())?;
        let state_machine = StateMachine::new(app_db, Arc::new(MockBlobParser))?;

        let root1 = B256::from([1u8; 32]);
        let root2 = B256::from([2u8; 32]);
        let parent = B256::from([9u8; 32]);
        let j1 = SlotJournal {
            slot: 30,
            tx_hashes: vec![unique_hash(30)],
            nullifiers: vec![unique_hash(31)],
            gsr_block_numbers: vec![3000],
            gsr_hashes: vec![unique_hash(32)],
        };
        sync_db
            .save_pending_slot(30, Some(root1), Some(parent), Some(3000), false, &j1)
            .await?;
        state_machine.apply_journal(&j1)?;
        sync_db.finalize_slot_applied(30, Some(3000)).await?;

        let j2 = SlotJournal {
            slot: 31,
            tx_hashes: vec![unique_hash(40)],
            nullifiers: vec![unique_hash(41)],
            gsr_block_numbers: vec![3001],
            gsr_hashes: vec![unique_hash(42)],
        };
        sync_db
            .save_pending_slot(31, Some(root2), Some(root1), Some(3001), false, &j2)
            .await?;
        state_machine.apply_journal(&j2)?;
        sync_db.finalize_slot_applied(31, Some(3001)).await?;
        state_machine.reload_from_db()?;

        // Stage rollback only; simulate crash before KV rollback/complete step.
        let staged = sync_db.rollback_to_slot(Some(30)).await?;
        assert_eq!(staged.len(), 1);

        // "Restart" recovery path should still see rollback work pending.
        let pending = sync_db.pending_recoveries().await?;
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].slot, 31);
        assert_eq!(pending[0].op, JournalOp::Rollback);

        // Replay what Node::recover_pending does for rollback entries.
        for recovery in pending {
            if recovery.op == JournalOp::Rollback {
                state_machine.rollback_journals(std::slice::from_ref(&recovery.journal))?;
                sync_db.complete_rollback(&[recovery.slot]).await?;
            }
        }

        assert!(sync_db.pending_recoveries().await?.is_empty());
        let progress = sync_db.last_progress().await?.expect("progress");
        assert_eq!(progress.last_processed_slot, 30);
        assert_eq!(progress.last_processed_block_number, Some(3000));
        assert_eq!(sync_db.slot_root(30).await?, Some(root1));
        assert_eq!(sync_db.slot_root(31).await?, None);

        let (txs, nullifiers, gsrs) = state_machine.state_snapshot()?;
        assert!(txs.contains(&j1.tx_hashes[0]));
        assert!(!txs.contains(&j2.tx_hashes[0]));
        assert!(nullifiers.contains(&j1.nullifiers[0]));
        assert!(!nullifiers.contains(&j2.nullifiers[0]));
        assert!(gsrs.contains(&j1.gsr_hashes[0]));
        assert!(!gsrs.contains(&j2.gsr_hashes[0]));

        drop(sync_db);
        drop_db(&admin_url, &db_name).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local postgres"]
    async fn test_empty_slot_finalize_sets_none_root_and_advances_cursor() -> Result<()> {
        let _guard = test_db_lock().lock().expect("lock");
        let (admin_url, db_url, db_name) = test_urls();
        drop_db(&admin_url, &db_name).await?;
        let sync_db = SyncDb::connect(&db_url).await?;

        let slot = 55;
        let journal = SlotJournal {
            slot,
            tx_hashes: vec![],
            nullifiers: vec![],
            gsr_block_numbers: vec![],
            gsr_hashes: vec![],
        };
        sync_db
            .save_pending_slot(slot, None, None, None, true, &journal)
            .await?;
        sync_db.finalize_slot_applied(slot, None).await?;

        assert_eq!(sync_db.slot_root(slot).await?, None);
        let progress = sync_db.last_progress().await?.expect("progress");
        assert_eq!(progress.last_processed_slot, slot);
        assert_eq!(progress.last_processed_block_number, None);

        drop(sync_db);
        drop_db(&admin_url, &db_name).await?;
        Ok(())
    }
}
