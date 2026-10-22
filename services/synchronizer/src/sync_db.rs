use std::collections::HashMap;

use alloy::primitives::B256;
use anyhow::{anyhow, Context, Result};
use pod2::middleware::Hash;
use sqlx::{
    postgres::{PgPoolOptions, PgRow},
    Executor, PgExecutor, PgPool, Row,
};

use crate::{
    don_state::{DonState, ExecutionBlock, StateMetadata, StateRoots},
    key_value_db::{db_bytes_to_hash, hash_to_db_bytes},
};

/// Head state plus progress metadata loaded from Postgres.
#[derive(Debug, Clone, Copy)]
pub struct HeadSnapshot {
    pub state: DonState,
    pub last_processed_slot: u32,
    pub last_processed_block_number: Option<u32>,
}

/// Consensus-layer metadata written when a slot is committed.
#[derive(Debug, Clone, Copy)]
pub struct CommittedSlotRecord {
    pub slot: u32,
    pub block_root: Option<B256>,
    pub parent_root: Option<B256>,
    pub is_empty: bool,
}

impl CommittedSlotRecord {
    /// Record for a slot with no block content: every block field
    /// absent, marked empty.
    pub fn empty(slot: u32) -> Self {
        Self {
            slot,
            block_root: None,
            parent_root: None,
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
    /// Postgres is the sole source of committed states. Each committed slot stores its
    /// `DonState`, while RocksDB only stores the content-addressed Merkle node/value backing store.
    ///
    /// Fails if the stored slots do not start at `init_start_slot`, which happens when the
    /// setting changed after the database was first synced.
    pub async fn connect(database_url: &str, init_start_slot: u32) -> Result<Self> {
        anyhow::ensure!(init_start_slot >= 1, "INIT_START_SLOT must be >= 1");
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(database_url)
            .await
            .with_context(|| "Failed to connect to sync metadata Postgres")?;
        let store = Self { pool };
        store.bootstrap().await?;
        if let Some(first_slot) = store.first_committed_slot().await? {
            anyhow::ensure!(
                first_slot == init_start_slot,
                "stored slots start at {first_slot} but INIT_START_SLOT is {init_start_slot}; \
                 resync from an empty database to change it"
            );
        }
        Ok(store)
    }

    /// Create the Postgres schema used by the synchronizer control plane.
    ///
    /// `slots` stores per-slot metadata plus, for every slot with an execution block, the state
    /// derived at that block. The highest slot with an execution block holds the head state.
    async fn bootstrap(&self) -> Result<()> {
        let statements = [
            r#"
            CREATE TABLE IF NOT EXISTS slots (
                slot INTEGER PRIMARY KEY,
                cl_block_root BYTEA NULL,
                cl_parent_root BYTEA NULL,
                el_block_number INTEGER NULL,
                el_block_timestamp BIGINT NULL,
                el_block_hash BYTEA NULL,
                cl_is_empty BOOLEAN NOT NULL,
                don_created_root BYTEA NULL,
                don_nullifiers_root BYTEA NULL,
                don_prior_state_history_root BYTEA NULL,
                don_next_state_history_root BYTEA NULL,
                don_state_root BYTEA NULL,
                don_created_count BIGINT NULL,
                don_nullifier_count BIGINT NULL,
                don_state_root_count BIGINT NULL,
                updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
                -- A slot derives a state exactly when it has an execution block.
                CHECK (num_nulls(
                    el_block_number,
                    el_block_timestamp,
                    el_block_hash,
                    don_created_root,
                    don_nullifiers_root,
                    don_prior_state_history_root,
                    don_next_state_history_root,
                    don_state_root,
                    don_created_count,
                    don_nullifier_count,
                    don_state_root_count
                ) IN (0, 11))
            )
            "#,
            r#"
            CREATE UNIQUE INDEX IF NOT EXISTS slots_cl_block_root_uq
                ON slots(cl_block_root)
                WHERE cl_block_root IS NOT NULL
            "#,
            r#"
            CREATE INDEX IF NOT EXISTS slots_el_block_number_idx
                ON slots(el_block_number)
                WHERE el_block_number IS NOT NULL
            "#,
            // Reverse index from object commitment to its position in the created
            // `Array`. A materialized view of committed state: rows are inserted in
            // the same transaction that commits their slot and deleted in the same
            // transaction that rolls the slot back, so the index never diverges
            // from the committed state. `slot` carries the rollback axis.
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
            FROM slots
            ORDER BY slot DESC
            LIMIT 1
            "#,
        )
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|row| row.get::<i32, _>("slot") as u32))
    }

    async fn first_committed_slot(&self) -> Result<Option<u32>> {
        let row = sqlx::query("SELECT MIN(slot) AS slot FROM slots")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.get::<Option<i32>, _>("slot").map(|slot| slot as u32))
    }

    /// Return the last slot fully committed by the synchronizer, or `None` before the first.
    pub async fn last_processed_slot(&self) -> Result<Option<u32>> {
        self.latest_committed_slot().await
    }

    /// Return the head state without sync-progress metadata, or `None` before the first slot.
    pub async fn head_state(&self) -> Result<Option<DonState>> {
        Ok(self.head_snapshot().await?.map(|snapshot| snapshot.state))
    }

    /// Return the head state plus sync progress from one Postgres snapshot, or `None` before
    /// the first slot.
    pub async fn head_snapshot(&self) -> Result<Option<HeadSnapshot>> {
        Self::read_snapshot(&self.pool).await
    }

    /// Read the head state plus progress over any executor (pool or
    /// transaction), so a caller can pin it to the same snapshot as other reads.
    async fn read_snapshot<'e, E: PgExecutor<'e>>(executor: E) -> Result<Option<HeadSnapshot>> {
        let row = sqlx::query(
            r#"
            WITH latest AS (
                SELECT slot, el_block_number
                FROM slots
                ORDER BY slot DESC
                LIMIT 1
            ),
            head AS (
                SELECT slot,
                       el_block_number,
                       el_block_timestamp,
                       el_block_hash,
                       don_created_root,
                       don_nullifiers_root,
                       don_prior_state_history_root,
                       don_next_state_history_root,
                       don_state_root,
                       don_created_count,
                       don_nullifier_count,
                       don_state_root_count
                FROM slots
                WHERE el_block_number IS NOT NULL
                ORDER BY slot DESC
                LIMIT 1
            )
            SELECT latest.slot AS last_processed_slot,
                   latest.el_block_number AS last_processed_block_number,
                   head.*
            FROM latest LEFT JOIN head ON true
            "#,
        )
        .fetch_optional(executor)
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };

        let last_processed_slot = row.get::<i32, _>("last_processed_slot") as u32;
        let last_processed_block_number = row
            .get::<Option<i32>, _>("last_processed_block_number")
            .map(|block_number| block_number as u32);

        // The first slot always has an execution block, so a non-empty table has a head state.
        let head_slot = row.get::<Option<i32>, _>("slot").ok_or_else(|| {
            anyhow!("slots has rows but none with an execution block, so no head state")
        })?;
        let state = decode_state_row(&row)
            .with_context(|| format!("slots row for slot {head_slot} had invalid state data"))?;

        Ok(Some(HeadSnapshot {
            state,
            last_processed_slot,
            last_processed_block_number,
        }))
    }

    /// Return the beacon block root for a committed slot.
    pub async fn slot_root(&self, slot: u32) -> Result<Option<B256>> {
        let row = sqlx::query("SELECT cl_block_root FROM slots WHERE slot = $1")
            .bind(slot as i32)
            .fetch_optional(&self.pool)
            .await?;

        let Some(row) = row else {
            return Ok(None);
        };
        let root: Option<Vec<u8>> = row.get("cl_block_root");
        match root {
            Some(bytes) => {
                let arr: [u8; 32] = bytes
                    .as_slice()
                    .try_into()
                    .map_err(|_| anyhow!("invalid cl_block_root length"))?;
                Ok(Some(B256::from(arr)))
            }
            None => Ok(None),
        }
    }

    /// Commit one new slot as the new highest slot, writing
    /// its created-index rows in the same transaction so the index is always
    /// consistent with the committed state.
    ///
    /// `derived_state` is the state the slot derived at its execution block, or `None` when the
    /// slot has no execution block and the previous state stays the head state.
    ///
    /// Duplicate slot inserts are treated as logic bugs and must fail loudly.
    pub async fn commit_slot(
        &self,
        slot: &CommittedSlotRecord,
        derived_state: Option<&DonState>,
        created_added: &HashMap<Hash, i64>,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            r#"
            INSERT INTO slots(
                slot,
                cl_block_root,
                cl_parent_root,
                el_block_number,
                el_block_timestamp,
                el_block_hash,
                cl_is_empty,
                don_created_root,
                don_nullifiers_root,
                don_prior_state_history_root,
                don_next_state_history_root,
                don_state_root,
                don_created_count,
                don_nullifier_count,
                don_state_root_count
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)
            "#,
        )
        .bind(slot.slot as i32)
        .bind(slot.block_root.map(|v| v.as_slice().to_vec()))
        .bind(slot.parent_root.map(|v| v.as_slice().to_vec()))
        .bind(derived_state.map(|state| state.metadata.block.number as i32))
        .bind(derived_state.map(|state| state.metadata.block.timestamp as i64))
        .bind(derived_state.map(|state| state.metadata.block.hash.as_slice().to_vec()))
        .bind(slot.is_empty)
        .bind(derived_state.map(|state| hash_to_db_bytes(state.roots.created)))
        .bind(derived_state.map(|state| hash_to_db_bytes(state.roots.nullifiers)))
        .bind(derived_state.map(|state| hash_to_db_bytes(state.roots.prior_state_history)))
        .bind(derived_state.map(|state| hash_to_db_bytes(state.roots.next_state_history)))
        .bind(derived_state.map(|state| hash_to_db_bytes(state.metadata.state_root)))
        .bind(derived_state.map(|state| state.metadata.created_count as i64))
        .bind(derived_state.map(|state| state.metadata.nullifier_count as i64))
        .bind(derived_state.map(|state| state.metadata.state_root_count as i64))
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

    /// Delete slots after `keep_slot`, leaving the highest remaining
    /// row as the head, and prune the created-index rows those slots added in the
    /// same transaction.
    pub async fn rollback_to_slot(&self, keep_slot: u32) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        sqlx::query("DELETE FROM slots WHERE slot > $1")
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

    /// Read the head snapshot and the created indices for `commitments` from
    /// one `REPEATABLE READ` transaction, so the state roots and the index rows
    /// reflect a single consistent Postgres snapshot. Without this a concurrent
    /// `commit_slot`/`rollback_to_slot` could land between the two reads and make
    /// the index inconsistent with the state a query is answering against. The
    /// RocksDB array read needs no coordination: nodes are content-addressed and
    /// immutable by root, so reading at the pinned root is stable regardless.
    pub async fn snapshot_with_created_indices(
        &self,
        commitments: &[Hash],
    ) -> Result<(Option<HeadSnapshot>, HashMap<Hash, i64>)> {
        let mut tx = self
            .pool
            .begin_with("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .await?;
        let snapshot = Self::read_snapshot(&mut *tx).await?;
        let indices = Self::read_created_indices(&mut *tx, commitments).await?;
        tx.commit().await?;
        Ok((snapshot, indices))
    }

    /// Return the recent state roots at or above the given execution block number.
    pub async fn recent_state_roots(
        &self,
        min_block_number: Option<u32>,
    ) -> Result<Vec<(Hash, i64)>> {
        let Some(min_block_number) = min_block_number else {
            return Ok(Vec::new());
        };

        let rows = sqlx::query(
            r#"
            SELECT don_state_root, el_block_number
            FROM slots
            WHERE el_block_number >= $1
            ORDER BY el_block_number ASC, slot ASC
            "#,
        )
        .bind(min_block_number as i32)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|row| {
                let state_root: Vec<u8> = row.get("don_state_root");
                let block_number = row.get::<i32, _>("el_block_number");
                Ok((db_bytes_to_hash(&state_root)?, i64::from(block_number)))
            })
            .collect()
    }
}

/// Decode the DON state from a row with an execution block, the block the state was derived at.
fn decode_state_row(row: &PgRow) -> Result<DonState> {
    let hash_bytes: Vec<u8> = row.get("el_block_hash");
    let hash: [u8; 32] = hash_bytes
        .as_slice()
        .try_into()
        .map_err(|_| anyhow!("invalid el_block_hash length"))?;
    let block = ExecutionBlock {
        number: row.get::<i32, _>("el_block_number") as u32,
        timestamp: row.get::<i64, _>("el_block_timestamp") as u64,
        hash: B256::from(hash),
    };
    Ok(DonState {
        roots: StateRoots {
            created: db_bytes_to_hash(&row.get::<Vec<u8>, _>("don_created_root"))?,
            nullifiers: db_bytes_to_hash(&row.get::<Vec<u8>, _>("don_nullifiers_root"))?,
            prior_state_history: db_bytes_to_hash(
                &row.get::<Vec<u8>, _>("don_prior_state_history_root"),
            )?,
            next_state_history: db_bytes_to_hash(
                &row.get::<Vec<u8>, _>("don_next_state_history_root"),
            )?,
        },
        metadata: StateMetadata {
            state_root: db_bytes_to_hash(&row.get::<Vec<u8>, _>("don_state_root"))?,
            block,
            created_count: row.get::<i64, _>("don_created_count") as u64,
            nullifier_count: row.get::<i64, _>("don_nullifier_count") as u64,
            state_root_count: row.get::<i64, _>("don_state_root_count") as u64,
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

    fn unique_execution_block(block_number: u32) -> ExecutionBlock {
        ExecutionBlock {
            number: block_number,
            timestamp: (block_number as u64) * 1000,
            hash: B256::repeat_byte(block_number as u8),
        }
    }

    fn unique_state(block_number: u32, marker: i64) -> DonState {
        DonState {
            roots: StateRoots {
                created: unique_hash(marker),
                nullifiers: unique_hash(marker + 1),
                prior_state_history: unique_hash(marker + 2),
                next_state_history: unique_hash(marker + 3),
            },
            metadata: StateMetadata {
                state_root: unique_hash(marker + 4),
                block: unique_execution_block(block_number),
                created_count: block_number as u64,
                nullifier_count: block_number as u64 + 1,
                state_root_count: block_number as u64 + 2,
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
        let sync_db = SyncDb::connect(&db_url, 1).await?;
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
    async fn test_empty_database_has_no_head() -> Result<()> {
        let (sync_db, db_name, admin_url) = fresh_sync_db().await?;

        assert_eq!(sync_db.last_processed_slot().await?, None);
        assert!(sync_db.head_snapshot().await?.is_none());

        drop_db(&db_name, &admin_url).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local postgres"]
    async fn test_connect_rejects_changed_init_start_slot() -> Result<()> {
        let (db_name, db_url, admin_url) = test_urls();
        create_db(&db_name, &admin_url).await?;
        let sync_db = SyncDb::connect(&db_url, 5).await?;
        sync_db
            .commit_slot(&CommittedSlotRecord::empty(5), None, &HashMap::new())
            .await?;

        assert!(SyncDb::connect(&db_url, 5).await.is_ok());
        assert!(SyncDb::connect(&db_url, 4).await.is_err());

        drop_db(&db_name, &admin_url).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local postgres"]
    async fn test_commit_slot_persists_state_and_latest_slot() -> Result<()> {
        let (sync_db, db_name, admin_url) = fresh_sync_db().await?;
        let state = unique_state(7, 100);
        let slot = CommittedSlotRecord {
            slot: 10,
            block_root: None,
            parent_root: None,
            is_empty: false,
        };

        sync_db
            .commit_slot(&slot, Some(&state), &HashMap::new())
            .await?;

        let snapshot = sync_db.head_snapshot().await?.expect("a slot is committed");
        assert_eq!(snapshot.state, state);
        assert_eq!(snapshot.last_processed_slot, 10);
        assert_eq!(snapshot.last_processed_block_number, Some(7));

        drop_db(&db_name, &admin_url).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local postgres"]
    async fn test_head_state_comes_from_last_slot_with_execution_block() -> Result<()> {
        let (sync_db, db_name, admin_url) = fresh_sync_db().await?;
        let state = unique_state(7, 100);
        let derived = CommittedSlotRecord {
            slot: 10,
            block_root: None,
            parent_root: None,
            is_empty: false,
        };
        sync_db
            .commit_slot(&derived, Some(&state), &HashMap::new())
            .await?;
        // Later slots without an execution block derive no state.
        let without_payload = CommittedSlotRecord {
            slot: 11,
            block_root: Some(B256::repeat_byte(11)),
            parent_root: None,
            is_empty: false,
        };
        sync_db
            .commit_slot(&without_payload, None, &HashMap::new())
            .await?;
        sync_db
            .commit_slot(&CommittedSlotRecord::empty(12), None, &HashMap::new())
            .await?;

        let snapshot = sync_db.head_snapshot().await?.expect("a slot is committed");
        assert_eq!(snapshot.state, state);
        assert_eq!(snapshot.last_processed_slot, 12);
        assert_eq!(snapshot.last_processed_block_number, None);

        drop_db(&db_name, &admin_url).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local postgres"]
    async fn test_commit_slot_duplicate_slot_fails() -> Result<()> {
        let (sync_db, db_name, admin_url) = fresh_sync_db().await?;
        let state1 = unique_state(7, 100);
        let state2 = unique_state(8, 200);
        let slot = CommittedSlotRecord {
            slot: 10,
            block_root: None,
            parent_root: None,
            is_empty: false,
        };

        sync_db
            .commit_slot(&slot, Some(&state1), &HashMap::new())
            .await?;

        let err = sync_db
            .commit_slot(&slot, Some(&state2), &HashMap::new())
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
    async fn test_recent_state_roots_query() -> Result<()> {
        let (sync_db, db_name, admin_url) = fresh_sync_db().await?;
        let h1 = unique_state(5, 10);
        sync_db
            .commit_slot(
                &CommittedSlotRecord {
                    slot: 1,
                    block_root: None,
                    parent_root: None,
                    is_empty: false,
                },
                Some(&h1),
                &HashMap::new(),
            )
            .await?;

        let h2 = unique_state(9, 20);
        sync_db
            .commit_slot(
                &CommittedSlotRecord {
                    slot: 2,
                    block_root: None,
                    parent_root: None,
                    is_empty: false,
                },
                Some(&h2),
                &HashMap::new(),
            )
            .await?;

        let recent = sync_db.recent_state_roots(Some(6)).await?;
        assert_eq!(recent, vec![(h2.metadata.state_root, 9)]);

        drop_db(&db_name, &admin_url).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local postgres"]
    async fn test_rollback_rewinds_latest_slot_and_state() -> Result<()> {
        let (sync_db, db_name, admin_url) = fresh_sync_db().await?;
        let h1 = unique_state(1, 10);
        sync_db
            .commit_slot(
                &CommittedSlotRecord {
                    slot: 1,
                    block_root: None,
                    parent_root: None,
                    is_empty: false,
                },
                Some(&h1),
                &HashMap::new(),
            )
            .await?;

        let h2 = unique_state(2, 20);
        sync_db
            .commit_slot(
                &CommittedSlotRecord {
                    slot: 2,
                    block_root: None,
                    parent_root: None,
                    is_empty: false,
                },
                Some(&h2),
                &HashMap::new(),
            )
            .await?;

        sync_db.rollback_to_slot(1).await?;

        let snapshot = sync_db.head_snapshot().await?.expect("a slot is committed");
        assert_eq!(snapshot.state, h1);
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

        let h1 = unique_state(1, 10);
        sync_db
            .commit_slot(
                &CommittedSlotRecord {
                    slot: 1,
                    block_root: None,
                    parent_root: None,
                    is_empty: false,
                },
                Some(&h1),
                &HashMap::from([(a, 1)]),
            )
            .await?;
        let h2 = unique_state(2, 20);
        sync_db
            .commit_slot(
                &CommittedSlotRecord {
                    slot: 2,
                    block_root: None,
                    parent_root: None,
                    is_empty: false,
                },
                Some(&h2),
                &HashMap::from([(b, 2)]),
            )
            .await?;

        let indices = sync_db.created_indices(&[a, b]).await?;
        assert_eq!(indices.get(&a), Some(&1));
        assert_eq!(indices.get(&b), Some(&2));

        // Rolling back to slot 1 prunes slot 2's index row in the same
        // transaction as the slot delete.
        sync_db.rollback_to_slot(1).await?;
        let indices = sync_db.created_indices(&[a, b]).await?;
        assert_eq!(indices.get(&a), Some(&1));
        assert_eq!(indices.get(&b), None);

        // The atomic read returns the same index against the head state.
        let (snapshot, indices) = sync_db.snapshot_with_created_indices(&[a, b]).await?;
        assert_eq!(
            snapshot.expect("a slot is committed").last_processed_slot,
            1
        );
        assert_eq!(indices.get(&a), Some(&1));
        assert_eq!(indices.get(&b), None);

        drop_db(&db_name, &admin_url).await?;
        Ok(())
    }
}
