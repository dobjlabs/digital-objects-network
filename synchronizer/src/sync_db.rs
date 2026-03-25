use alloy::primitives::B256;
use anyhow::{anyhow, Context, Result};
use pod2::middleware::Hash;
use sqlx::{
    postgres::{PgPoolOptions, PgRow},
    Executor, PgPool, Row,
};
use url::Url;

use crate::{
    app_db::{db_bytes_to_hash, hash_to_db_bytes},
    head::{CanonicalHead, CanonicalRoots, HeadMetadata},
};

/// Current canonical head plus cursor metadata loaded from Postgres.
#[derive(Debug, Clone, Copy)]
pub struct CurrentSnapshot {
    pub head: CanonicalHead,
    pub last_processed_slot: Option<u32>,
    pub last_processed_block_number: Option<u32>,
}

/// Canonical slot metadata written when a slot is committed.
#[derive(Debug, Clone, Copy)]
pub struct CommittedSlotRecord {
    pub slot: u32,
    pub block_root: Option<B256>,
    pub parent_root: Option<B256>,
    pub block_number: Option<u32>,
    pub current_gsr: Option<Hash>,
    pub is_empty: bool,
}

#[derive(Clone)]
pub struct SyncDb {
    pool: PgPool,
}

impl SyncDb {
    /// Connect to the synchronizer's Postgres metadata database.
    ///
    /// Postgres is the sole source of canonical heads. Each committed slot stores its
    /// `CanonicalHead`,
    /// while RocksDB only stores the content-addressed Merkle node/value backing store.
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

    /// Create the Postgres schema used by the synchronizer control plane.
    ///
    /// `canonical_slots` stores per-slot metadata plus the committed canonical roots and metadata
    /// as regular SQL columns.
    /// `sync_cursor` points at the latest canonical slot that should be treated as current.
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
                head_transactions_root BYTEA NOT NULL,
                head_nullifiers_root BYTEA NOT NULL,
                head_state_root_gsrs_root BYTEA NOT NULL,
                head_gsr_history_root BYTEA NOT NULL,
                head_current_gsr BYTEA NULL,
                head_current_block_number INTEGER NULL,
                head_tx_count BIGINT NOT NULL,
                head_nullifier_count BIGINT NOT NULL,
                head_gsr_count BIGINT NOT NULL,
                updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
            )
            "#,
            r#"
            CREATE UNIQUE INDEX IF NOT EXISTS canonical_slots_block_root_uq
                ON canonical_slots(block_root)
                WHERE block_root IS NOT NULL
            "#,
            r#"
            CREATE INDEX IF NOT EXISTS canonical_slots_gsr_block_idx
                ON canonical_slots(execution_block_number)
                WHERE current_gsr IS NOT NULL AND execution_block_number IS NOT NULL
            "#,
        ];

        for stmt in statements {
            self.pool.execute(stmt).await?;
        }

        Ok(())
    }

    /// Ensure the single sync-cursor row exists and return the next slot the node should process.
    ///
    /// On first run, starts from `initial_start` when provided, otherwise current head slot.
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

    /// Return the last canonical slot fully committed by the synchronizer.
    pub async fn last_processed_slot(&self) -> Result<Option<u32>> {
        let row = sqlx::query("SELECT last_processed_slot FROM sync_cursor WHERE id = 1")
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.and_then(|r| {
            r.get::<Option<i32>, _>("last_processed_slot")
                .map(|s| s as u32)
        }))
    }

    /// Return the current canonical head without sync-progress metadata.
    pub async fn current_head(&self) -> Result<CanonicalHead> {
        Ok(self.current_snapshot().await?.head)
    }

    /// Return the current canonical head plus sync progress from one Postgres snapshot.
    pub async fn current_snapshot(&self) -> Result<CurrentSnapshot> {
        let row = sqlx::query(
            r#"
            SELECT c.last_processed_slot,
                   c.last_processed_block_number,
                   s.head_transactions_root,
                   s.head_nullifiers_root,
                   s.head_state_root_gsrs_root,
                   s.head_gsr_history_root,
                   s.head_current_gsr,
                   s.head_current_block_number,
                   s.head_tx_count,
                   s.head_nullifier_count,
                   s.head_gsr_count
            FROM sync_cursor c
            LEFT JOIN canonical_slots s ON s.slot = c.last_processed_slot
            WHERE c.id = 1
            "#,
        )
        .fetch_optional(&self.pool)
        .await?;

        let Some(row) = row else {
            return Ok(CurrentSnapshot {
                head: CanonicalHead::empty(),
                last_processed_slot: None,
                last_processed_block_number: None,
            });
        };

        let last_processed_slot = row
            .get::<Option<i32>, _>("last_processed_slot")
            .map(|slot| slot as u32);
        let last_processed_block_number = row
            .get::<Option<i32>, _>("last_processed_block_number")
            .map(|block_number| block_number as u32);

        let head = match last_processed_slot {
            None => CanonicalHead::empty(),
            Some(slot) => decode_head_row(&row).with_context(|| {
                format!("sync_cursor points at slot {slot}, but canonical_slots has no head data")
            })?,
        };

        Ok(CurrentSnapshot {
            head,
            last_processed_slot,
            last_processed_block_number,
        })
    }

    /// Return the committed canonical head for one slot, if present.
    #[cfg(test)]
    pub async fn head_for_slot(&self, slot: u32) -> Result<Option<CanonicalHead>> {
        let row = sqlx::query(
            r#"
            SELECT head_transactions_root,
                   head_nullifiers_root,
                   head_state_root_gsrs_root,
                   head_gsr_history_root,
                   head_current_gsr,
                   head_current_block_number,
                   head_tx_count,
                   head_nullifier_count,
                   head_gsr_count
            FROM canonical_slots
            WHERE slot = $1
            "#,
        )
        .bind(slot as i32)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| decode_head_row(&row)).transpose()
    }

    /// Return the canonical beacon block root for a committed slot.
    pub async fn slot_root(&self, slot: u32) -> Result<Option<B256>> {
        let row = sqlx::query("SELECT block_root FROM canonical_slots WHERE slot = $1")
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

    /// Commit one canonical slot and advance the sync cursor in the same Postgres transaction.
    pub async fn commit_slot(
        &self,
        slot: &CommittedSlotRecord,
        head: &CanonicalHead,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            r#"
            INSERT INTO canonical_slots(
                slot,
                block_root,
                parent_root,
                execution_block_number,
                current_gsr,
                is_empty,
                head_transactions_root,
                head_nullifiers_root,
                head_state_root_gsrs_root,
                head_gsr_history_root,
                head_current_gsr,
                head_current_block_number,
                head_tx_count,
                head_nullifier_count,
                head_gsr_count
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
            ON CONFLICT (slot) DO UPDATE
            SET block_root = EXCLUDED.block_root,
                parent_root = EXCLUDED.parent_root,
                execution_block_number = EXCLUDED.execution_block_number,
                current_gsr = EXCLUDED.current_gsr,
                is_empty = EXCLUDED.is_empty,
                head_transactions_root = EXCLUDED.head_transactions_root,
                head_nullifiers_root = EXCLUDED.head_nullifiers_root,
                head_state_root_gsrs_root = EXCLUDED.head_state_root_gsrs_root,
                head_gsr_history_root = EXCLUDED.head_gsr_history_root,
                head_current_gsr = EXCLUDED.head_current_gsr,
                head_current_block_number = EXCLUDED.head_current_block_number,
                head_tx_count = EXCLUDED.head_tx_count,
                head_nullifier_count = EXCLUDED.head_nullifier_count,
                head_gsr_count = EXCLUDED.head_gsr_count,
                updated_at = now()
            "#,
        )
        .bind(slot.slot as i32)
        .bind(slot.block_root.map(|v| v.as_slice().to_vec()))
        .bind(slot.parent_root.map(|v| v.as_slice().to_vec()))
        .bind(slot.block_number.map(|v| v as i32))
        .bind(slot.current_gsr.map(hash_to_db_bytes))
        .bind(slot.is_empty)
        .bind(hash_to_db_bytes(head.roots.transactions))
        .bind(hash_to_db_bytes(head.roots.nullifiers))
        .bind(hash_to_db_bytes(head.roots.state_root_gsrs))
        .bind(hash_to_db_bytes(head.roots.gsr_history))
        .bind(head.metadata.current_gsr.map(hash_to_db_bytes))
        .bind(head.metadata.current_block_number.map(|v| v as i32))
        .bind(head.metadata.tx_count as i64)
        .bind(head.metadata.nullifier_count as i64)
        .bind(head.metadata.gsr_count as i64)
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
        .bind(slot.slot as i32)
        .bind(slot.block_number.map(|v| v as i32))
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }

    /// Delete canonical slots after `keep_slot` and rewind the sync cursor in one transaction.
    pub async fn rollback_to_slot(&self, keep_slot: u32) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        sqlx::query("DELETE FROM canonical_slots WHERE slot > $1")
            .bind(keep_slot as i32)
            .execute(&mut *tx)
            .await?;

        sqlx::query(
            r#"
            WITH cursor_target AS (
                SELECT slot, execution_block_number
                FROM canonical_slots
                WHERE slot <= $1
                ORDER BY slot DESC
                LIMIT 1
            )
            INSERT INTO sync_cursor (id, last_processed_slot, last_processed_block_number)
            VALUES (
                1,
                (SELECT slot FROM cursor_target),
                (SELECT execution_block_number FROM cursor_target)
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
        Ok(())
    }

    /// Return the recent canonical GSRs at or above the given execution block number.
    pub async fn recent_gsrs(&self, min_block_number: Option<u32>) -> Result<Vec<(Hash, i64)>> {
        let Some(min_block_number) = min_block_number else {
            return Ok(Vec::new());
        };

        let rows = sqlx::query(
            r#"
            SELECT current_gsr, execution_block_number
            FROM canonical_slots
            WHERE current_gsr IS NOT NULL
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

fn decode_head_row(row: &PgRow) -> Result<CanonicalHead> {
    Ok(CanonicalHead {
        roots: CanonicalRoots {
            transactions: db_bytes_to_hash(&row.get::<Vec<u8>, _>("head_transactions_root"))?,
            nullifiers: db_bytes_to_hash(&row.get::<Vec<u8>, _>("head_nullifiers_root"))?,
            state_root_gsrs: db_bytes_to_hash(&row.get::<Vec<u8>, _>("head_state_root_gsrs_root"))?,
            gsr_history: db_bytes_to_hash(&row.get::<Vec<u8>, _>("head_gsr_history_root"))?,
        },
        metadata: HeadMetadata {
            current_gsr: row
                .get::<Option<Vec<u8>>, _>("head_current_gsr")
                .as_deref()
                .map(db_bytes_to_hash)
                .transpose()?,
            current_block_number: row
                .get::<Option<i32>, _>("head_current_block_number")
                .map(|value| value as u32),
            tx_count: row.get::<i64, _>("head_tx_count") as u64,
            nullifier_count: row.get::<i64, _>("head_nullifier_count") as u64,
            gsr_count: row.get::<i64, _>("head_gsr_count") as u64,
        },
    })
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
    use pod2::middleware::{hash_values, Value};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_hash(n: i64) -> Hash {
        hash_values(&[Value::from(n)])
    }

    fn unique_head(block_number: u32, marker: i64) -> CanonicalHead {
        CanonicalHead {
            roots: CanonicalRoots {
                transactions: unique_hash(marker),
                nullifiers: unique_hash(marker + 1),
                state_root_gsrs: unique_hash(marker + 2),
                gsr_history: unique_hash(marker + 3),
            },
            metadata: HeadMetadata {
                current_gsr: Some(unique_hash(marker + 4)),
                current_block_number: Some(block_number),
                tx_count: block_number as u64,
                nullifier_count: block_number as u64 + 1,
                gsr_count: block_number as u64 + 2,
            },
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
    async fn test_current_snapshot_defaults_to_empty_head() -> Result<()> {
        let (sync_db, db_name, admin_url) = fresh_sync_db().await?;
        let snapshot = sync_db.current_snapshot().await?;
        assert_eq!(snapshot.head, CanonicalHead::empty());
        assert_eq!(snapshot.last_processed_slot, None);
        assert_eq!(snapshot.last_processed_block_number, None);
        drop_db(&db_name, &admin_url).await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_commit_slot_persists_head_and_cursor() -> Result<()> {
        let (sync_db, db_name, admin_url) = fresh_sync_db().await?;
        let head = unique_head(7, 100);
        let slot = CommittedSlotRecord {
            slot: 10,
            block_root: None,
            parent_root: None,
            block_number: Some(7),
            current_gsr: head.metadata.current_gsr,
            is_empty: false,
        };

        sync_db.commit_slot(&slot, &head).await?;

        let snapshot = sync_db.current_snapshot().await?;
        assert_eq!(snapshot.head, head);
        assert_eq!(snapshot.last_processed_slot, Some(10));
        assert_eq!(snapshot.last_processed_block_number, Some(7));
        assert_eq!(sync_db.head_for_slot(10).await?, Some(head));

        drop_db(&db_name, &admin_url).await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_recent_gsrs_query() -> Result<()> {
        let (sync_db, db_name, admin_url) = fresh_sync_db().await?;
        let h1 = unique_head(5, 10);
        sync_db
            .commit_slot(
                &CommittedSlotRecord {
                    slot: 1,
                    block_root: None,
                    parent_root: None,
                    block_number: Some(5),
                    current_gsr: h1.metadata.current_gsr,
                    is_empty: false,
                },
                &h1,
            )
            .await?;

        let h2 = unique_head(9, 20);
        sync_db
            .commit_slot(
                &CommittedSlotRecord {
                    slot: 2,
                    block_root: None,
                    parent_root: None,
                    block_number: Some(9),
                    current_gsr: h2.metadata.current_gsr,
                    is_empty: false,
                },
                &h2,
            )
            .await?;

        let recent = sync_db.recent_gsrs(Some(6)).await?;
        assert_eq!(recent, vec![(h2.metadata.current_gsr.unwrap(), 9)]);

        drop_db(&db_name, &admin_url).await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_rollback_rewinds_cursor_and_head() -> Result<()> {
        let (sync_db, db_name, admin_url) = fresh_sync_db().await?;
        let h1 = unique_head(1, 10);
        sync_db
            .commit_slot(
                &CommittedSlotRecord {
                    slot: 1,
                    block_root: None,
                    parent_root: None,
                    block_number: Some(1),
                    current_gsr: h1.metadata.current_gsr,
                    is_empty: false,
                },
                &h1,
            )
            .await?;

        let h2 = unique_head(2, 20);
        sync_db
            .commit_slot(
                &CommittedSlotRecord {
                    slot: 2,
                    block_root: None,
                    parent_root: None,
                    block_number: Some(2),
                    current_gsr: h2.metadata.current_gsr,
                    is_empty: false,
                },
                &h2,
            )
            .await?;

        sync_db.rollback_to_slot(1).await?;

        let snapshot = sync_db.current_snapshot().await?;
        assert_eq!(snapshot.head, h1);
        assert_eq!(snapshot.last_processed_slot, Some(1));
        assert_eq!(sync_db.head_for_slot(2).await?, None);

        drop_db(&db_name, &admin_url).await?;
        Ok(())
    }
}
