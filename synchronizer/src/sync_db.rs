use std::collections::HashMap;

use alloy::primitives::B256;
use anyhow::{anyhow, Context, Result};
use pod2::middleware::Hash;
use sqlx::{
    postgres::{PgPoolOptions, PgRow},
    Executor, PgExecutor, PgPool, Row,
};

use crate::{
    app_db::{db_bytes_to_hash, hash_to_db_bytes},
    head::{CanonicalHead, CanonicalRoots, HeadMetadata},
};

/// Current canonical head plus progress metadata loaded from Postgres.
#[derive(Debug, Clone, Copy)]
pub struct CurrentSnapshot {
    pub head: CanonicalHead,
    pub last_processed_slot: u32,
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

impl CommittedSlotRecord {
    /// Record for a slot with no canonical block content: every block field
    /// absent, marked empty.
    pub fn empty(slot: u32) -> Self {
        Self {
            slot,
            block_root: None,
            parent_root: None,
            block_number: None,
            current_gsr: None,
            is_empty: true,
        }
    }
}

#[derive(Clone)]
pub struct SyncDb {
    pool: PgPool,
}

impl SyncDb {
    /// Connect to the synchronizer's Postgres metadata database.
    ///
    /// The database must already exist — provisioning is an explicit operator
    /// step so the app role does not need the `CREATEDB` privilege.
    ///
    /// Postgres is the sole source of canonical heads. Each committed slot stores its
    /// `CanonicalHead`,
    /// while RocksDB only stores the content-addressed Merkle node/value backing store.
    pub async fn connect(database_url: &str) -> Result<Self> {
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
    /// as regular SQL columns. The highest committed slot is the current canonical head.
    async fn bootstrap(&self) -> Result<()> {
        let statements = [
            r#"
            CREATE TABLE IF NOT EXISTS canonical_slots (
                slot INTEGER PRIMARY KEY,
                block_root BYTEA NULL,
                parent_root BYTEA NULL,
                execution_block_number INTEGER NULL,
                current_gsr BYTEA NULL,
                is_empty BOOLEAN NOT NULL,
                head_created_root BYTEA NOT NULL,
                head_nullifiers_root BYTEA NOT NULL,
                head_state_root_gsrs_root BYTEA NOT NULL,
                head_gsr_history_root BYTEA NOT NULL,
                head_current_gsr BYTEA NULL,
                head_current_block_number INTEGER NULL,
                head_created_count BIGINT NOT NULL,
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
            // Reverse index from object commitment to its position in the created
            // `Array`. A materialized view of committed state: rows are inserted in
            // the same transaction that commits their slot and deleted in the same
            // transaction that rolls the slot back, so the index never diverges
            // from the canonical head. `slot` carries the rollback axis.
            r#"
            CREATE TABLE IF NOT EXISTS created_index (
                commitment BYTEA PRIMARY KEY,
                array_index BIGINT NOT NULL,
                slot INTEGER NOT NULL
            )
            "#,
            r#"
            CREATE INDEX IF NOT EXISTS created_index_slot_idx
                ON created_index(slot)
            "#,
        ];

        for stmt in statements {
            self.pool.execute(stmt).await?;
        }

        Ok(())
    }

    async fn latest_committed_slot(&self) -> Result<Option<u32>> {
        let row = sqlx::query(
            r#"
            SELECT slot
            FROM canonical_slots
            ORDER BY slot DESC
            LIMIT 1
            "#,
        )
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|row| row.get::<i32, _>("slot") as u32))
    }

    /// Insert the bootstrap canonical row when the database is still empty, and return the
    /// highest committed canonical slot afterward.
    pub async fn ensure_bootstrap_row(&self, bootstrap_slot: CommittedSlotRecord) -> Result<u32> {
        if let Some(stored_last) = self.latest_committed_slot().await? {
            return Ok(stored_last);
        }

        self.commit_slot(&bootstrap_slot, &CanonicalHead::empty(), &HashMap::new())
            .await?;
        Ok(bootstrap_slot.slot)
    }

    /// Return the last canonical slot fully committed by the synchronizer.
    pub async fn last_processed_slot(&self) -> Result<u32> {
        self.latest_committed_slot()
            .await?
            .ok_or_else(|| anyhow!("sync metadata not initialized"))
    }

    /// Return the current canonical head without sync-progress metadata.
    pub async fn current_head(&self) -> Result<CanonicalHead> {
        Ok(self.current_snapshot().await?.head)
    }

    /// Return the current canonical head plus sync progress from one Postgres snapshot.
    pub async fn current_snapshot(&self) -> Result<CurrentSnapshot> {
        Self::read_snapshot(&self.pool).await
    }

    /// Read the current canonical head plus progress over any executor (pool or
    /// transaction), so a caller can pin it to the same snapshot as other reads.
    async fn read_snapshot<'e, E: PgExecutor<'e>>(executor: E) -> Result<CurrentSnapshot> {
        let row = sqlx::query(
            r#"
            SELECT slot,
                   execution_block_number,
                   head_created_root,
                   head_nullifiers_root,
                   head_state_root_gsrs_root,
                   head_gsr_history_root,
                   head_current_gsr,
                   head_current_block_number,
                   head_created_count,
                   head_nullifier_count,
                   head_gsr_count
            FROM canonical_slots
            ORDER BY slot DESC
            LIMIT 1
            "#,
        )
        .fetch_optional(executor)
        .await?;

        let row = row.ok_or_else(|| anyhow!("sync metadata not initialized"))?;

        let last_processed_slot = row.get::<i32, _>("slot") as u32;
        let last_processed_block_number = row
            .get::<Option<i32>, _>("execution_block_number")
            .map(|block_number| block_number as u32);

        let head = decode_head_row(&row).with_context(|| {
            format!("canonical_slots row for slot {last_processed_slot} had invalid head data")
        })?;

        Ok(CurrentSnapshot {
            head,
            last_processed_slot,
            last_processed_block_number,
        })
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

    /// Commit one new canonical slot as the new highest canonical slot, writing
    /// its created-index rows in the same transaction so the index is always
    /// consistent with the committed head.
    ///
    /// Duplicate slot inserts are treated as logic bugs and must fail loudly.
    pub async fn commit_slot(
        &self,
        slot: &CommittedSlotRecord,
        head: &CanonicalHead,
        created_added: &HashMap<Hash, i64>,
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
                head_created_root,
                head_nullifiers_root,
                head_state_root_gsrs_root,
                head_gsr_history_root,
                head_current_gsr,
                head_current_block_number,
                head_created_count,
                head_nullifier_count,
                head_gsr_count
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
            "#,
        )
        .bind(slot.slot as i32)
        .bind(slot.block_root.map(|v| v.as_slice().to_vec()))
        .bind(slot.parent_root.map(|v| v.as_slice().to_vec()))
        .bind(slot.block_number.map(|v| v as i32))
        .bind(slot.current_gsr.map(hash_to_db_bytes))
        .bind(slot.is_empty)
        .bind(hash_to_db_bytes(head.roots.created))
        .bind(hash_to_db_bytes(head.roots.nullifiers))
        .bind(hash_to_db_bytes(head.roots.state_root_gsrs))
        .bind(hash_to_db_bytes(head.roots.gsr_history))
        .bind(head.metadata.current_gsr.map(hash_to_db_bytes))
        .bind(head.metadata.current_block_number.map(|v| v as i32))
        .bind(head.metadata.created_count as i64)
        .bind(head.metadata.nullifier_count as i64)
        .bind(head.metadata.gsr_count as i64)
        .execute(&mut *tx)
        .await?;

        for (commitment, index) in created_added {
            // `commitment` is the PRIMARY KEY; a conflict is a bug and fails loudly.
            sqlx::query(
                "INSERT INTO created_index(commitment, array_index, slot) VALUES ($1, $2, $3)",
            )
            .bind(hash_to_db_bytes(*commitment))
            .bind(*index)
            .bind(slot.slot as i32)
            .execute(&mut *tx)
            .await
            .with_context(|| {
                format!(
                    "inserting created_index row for {commitment:#} at slot {}",
                    slot.slot
                )
            })?;
        }

        tx.commit().await?;
        Ok(())
    }

    /// Delete canonical slots after `keep_slot`, leaving the highest remaining
    /// row as current, and prune the created-index rows those slots added in the
    /// same transaction.
    pub async fn rollback_to_slot(&self, keep_slot: u32) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        sqlx::query("DELETE FROM canonical_slots WHERE slot > $1")
            .bind(keep_slot as i32)
            .execute(&mut *tx)
            .await?;

        sqlx::query("DELETE FROM created_index WHERE slot > $1")
            .bind(keep_slot as i32)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(())
    }

    /// Map each known object commitment to its position in the created `Array`.
    /// Commitments absent from the index are omitted; a membership read treats a
    /// missing entry as "not present". Used both by the read API and as the
    /// derivation-time existence prefetch (where the index is cross-checked
    /// against the array before a commitment is treated as already created).
    pub async fn created_indices(&self, commitments: &[Hash]) -> Result<HashMap<Hash, i64>> {
        Self::read_created_indices(&self.pool, commitments).await
    }

    /// Read the created indices for `commitments` over any executor (pool or
    /// transaction), so a caller can pin them to the same snapshot as other reads.
    async fn read_created_indices<'e, E: PgExecutor<'e>>(
        executor: E,
        commitments: &[Hash],
    ) -> Result<HashMap<Hash, i64>> {
        if commitments.is_empty() {
            return Ok(HashMap::new());
        }
        let keys: Vec<Vec<u8>> = commitments.iter().map(|c| hash_to_db_bytes(*c)).collect();
        let rows = sqlx::query(
            "SELECT commitment, array_index FROM created_index WHERE commitment = ANY($1)",
        )
        .bind(keys)
        .fetch_all(executor)
        .await?;

        let mut map = HashMap::with_capacity(rows.len());
        for row in rows {
            let commitment: Vec<u8> = row.get("commitment");
            let index: i64 = row.get("array_index");
            map.insert(db_bytes_to_hash(&commitment)?, index);
        }
        Ok(map)
    }

    /// Read the current snapshot and the created indices for `commitments` from
    /// one `REPEATABLE READ` transaction, so the head roots and the index rows
    /// reflect a single consistent Postgres snapshot. Without this a concurrent
    /// `commit_slot`/`rollback_to_slot` could land between the two reads and make
    /// the index inconsistent with the head a query is answering against. The
    /// RocksDB array read needs no coordination: nodes are content-addressed and
    /// immutable by root, so reading at the pinned root is stable regardless.
    pub async fn snapshot_with_created_indices(
        &self,
        commitments: &[Hash],
    ) -> Result<(CurrentSnapshot, HashMap<Hash, i64>)> {
        let mut tx = self
            .pool
            .begin_with("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .await?;
        let snapshot = Self::read_snapshot(&mut *tx).await?;
        let indices = Self::read_created_indices(&mut *tx, commitments).await?;
        tx.commit().await?;
        Ok((snapshot, indices))
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
            created: db_bytes_to_hash(&row.get::<Vec<u8>, _>("head_created_root"))?,
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
            created_count: row.get::<i64, _>("head_created_count") as u64,
            nullifier_count: row.get::<i64, _>("head_nullifier_count") as u64,
            gsr_count: row.get::<i64, _>("head_gsr_count") as u64,
        },
    })
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
                created: unique_hash(marker),
                nullifiers: unique_hash(marker + 1),
                state_root_gsrs: unique_hash(marker + 2),
                gsr_history: unique_hash(marker + 3),
            },
            metadata: HeadMetadata {
                current_gsr: Some(unique_hash(marker + 4)),
                current_block_number: Some(block_number),
                created_count: block_number as u64,
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

    async fn create_db(db_name: &str, admin_url: &str) -> Result<()> {
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(admin_url)
            .await?;
        let escaped = db_name.replace('"', "\"\"");
        sqlx::query(&format!("CREATE DATABASE \"{escaped}\""))
            .execute(&pool)
            .await?;
        Ok(())
    }

    async fn fresh_sync_db() -> Result<(SyncDb, String, String)> {
        let (db_name, db_url, admin_url) = test_urls();
        create_db(&db_name, &admin_url).await?;
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
    #[ignore = "requires local postgres"]
    async fn test_bootstrap_inserts_bootstrap_row() -> Result<()> {
        let (sync_db, db_name, admin_url) = fresh_sync_db().await?;
        let bootstrap_slot = CommittedSlotRecord {
            slot: 4,
            block_root: None,
            parent_root: None,
            block_number: None,
            current_gsr: None,
            is_empty: true,
        };
        let start_slot = sync_db
            .ensure_bootstrap_row(bootstrap_slot)
            .await?
            .checked_add(1)
            .ok_or_else(|| anyhow!("last processed slot overflow"))?;
        assert_eq!(start_slot, 5);
        let snapshot = sync_db.current_snapshot().await?;
        assert_eq!(snapshot.head, CanonicalHead::empty());
        assert_eq!(snapshot.last_processed_slot, 4);
        assert_eq!(snapshot.last_processed_block_number, None);
        drop_db(&db_name, &admin_url).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local postgres"]
    async fn test_commit_slot_persists_head_and_latest_slot() -> Result<()> {
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

        sync_db.commit_slot(&slot, &head, &HashMap::new()).await?;

        let snapshot = sync_db.current_snapshot().await?;
        assert_eq!(snapshot.head, head);
        assert_eq!(snapshot.last_processed_slot, 10);
        assert_eq!(snapshot.last_processed_block_number, Some(7));

        drop_db(&db_name, &admin_url).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local postgres"]
    async fn test_commit_slot_duplicate_slot_fails() -> Result<()> {
        let (sync_db, db_name, admin_url) = fresh_sync_db().await?;
        let head1 = unique_head(7, 100);
        let head2 = unique_head(8, 200);
        let slot = CommittedSlotRecord {
            slot: 10,
            block_root: None,
            parent_root: None,
            block_number: Some(7),
            current_gsr: head1.metadata.current_gsr,
            is_empty: false,
        };

        sync_db.commit_slot(&slot, &head1, &HashMap::new()).await?;

        let err = sync_db
            .commit_slot(&slot, &head2, &HashMap::new())
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("duplicate key")
                || err.to_string().contains("unique constraint"),
            "unexpected duplicate-slot error: {err}"
        );

        drop_db(&db_name, &admin_url).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local postgres"]
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
                &HashMap::new(),
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
                &HashMap::new(),
            )
            .await?;

        let recent = sync_db.recent_gsrs(Some(6)).await?;
        assert_eq!(recent, vec![(h2.metadata.current_gsr.unwrap(), 9)]);

        drop_db(&db_name, &admin_url).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local postgres"]
    async fn test_rollback_rewinds_latest_slot_and_head() -> Result<()> {
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
                &HashMap::new(),
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
                &HashMap::new(),
            )
            .await?;

        sync_db.rollback_to_slot(1).await?;

        let snapshot = sync_db.current_snapshot().await?;
        assert_eq!(snapshot.head, h1);
        assert_eq!(snapshot.last_processed_slot, 1);

        drop_db(&db_name, &admin_url).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local postgres"]
    async fn test_created_index_commit_and_rollback() -> Result<()> {
        let (sync_db, db_name, admin_url) = fresh_sync_db().await?;
        let a = unique_hash(1);
        let b = unique_hash(2);

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
                &HashMap::from([(a, 1)]),
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
                &HashMap::from([(b, 2)]),
            )
            .await?;

        let indices = sync_db.created_indices(&[a, b]).await?;
        assert_eq!(indices.get(&a), Some(&1));
        assert_eq!(indices.get(&b), Some(&2));

        // Rolling back to slot 1 prunes slot 2's index row in the same
        // transaction as the canonical-slot delete.
        sync_db.rollback_to_slot(1).await?;
        let indices = sync_db.created_indices(&[a, b]).await?;
        assert_eq!(indices.get(&a), Some(&1));
        assert_eq!(indices.get(&b), None);

        // The atomic read returns the same index against the current head.
        let (snapshot, indices) = sync_db.snapshot_with_created_indices(&[a, b]).await?;
        assert_eq!(snapshot.last_processed_slot, 1);
        assert_eq!(indices.get(&a), Some(&1));
        assert_eq!(indices.get(&b), None);

        drop_db(&db_name, &admin_url).await?;
        Ok(())
    }
}
