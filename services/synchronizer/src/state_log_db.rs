use std::collections::HashMap;

use alloy::{eips::BlockHashOrNumber, primitives::B256};
use anyhow::{anyhow, Context, Result};
use pod2::middleware::Hash;
use sqlx::{
    postgres::{PgPoolOptions, PgRow},
    Executor, PgExecutor, PgPool, Row,
};

use crate::{
    container_db::{db_bytes_to_hash, hash_to_db_bytes},
    don_state::{DonState, ExecutionBlock, StateMetadata, StateRoots},
    state_machine::DerivedSlot,
};

/// A state loaded from Postgres with the slot whose execution block it was derived at.
#[derive(Debug, Clone, Copy)]
pub struct SlotState {
    pub state: DonState,
    pub slot: u32,
}

const SLOT_STATE_COLUMNS: &str = r#"
    slot,
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
"#;

#[derive(Clone)]
pub struct StateLogDb {
    pool: PgPool,
}

impl StateLogDb {
    /// Connect to the synchronizer's Postgres metadata database.
    ///
    /// The database must already exist: provisioning is an explicit operator
    /// step so the app role does not need the `CREATEDB` privilege.
    ///
    /// Fails if the stored slots do not start at `init_start_slot`, which happens when the
    /// setting changed after the database was first synced.
    pub async fn connect(database_url: &str, init_start_slot: u32) -> Result<Self> {
        anyhow::ensure!(init_start_slot >= 1, "INIT_START_SLOT must be >= 1");
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(database_url)
            .await
            .with_context(|| "Failed to connect to the state log Postgres")?;
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
                el_block_number BIGINT NULL,
                el_block_timestamp BIGINT NULL,
                el_block_hash BYTEA NULL,
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
            r#"
            CREATE INDEX IF NOT EXISTS slots_el_block_hash_idx
                ON slots(el_block_hash)
                WHERE el_block_hash IS NOT NULL
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

    async fn first_committed_slot(&self) -> Result<Option<u32>> {
        let slot: Option<i32> = sqlx::query_scalar("SELECT MIN(slot) FROM slots")
            .fetch_one(&self.pool)
            .await?;
        Ok(slot.map(|slot| slot as u32))
    }

    /// Return the last committed slot, or `None` before the first.
    pub async fn last_processed_slot(&self) -> Result<Option<u32>> {
        let slot: Option<i32> = sqlx::query_scalar("SELECT MAX(slot) FROM slots")
            .fetch_one(&self.pool)
            .await?;
        Ok(slot.map(|slot| slot as u32))
    }

    /// Return the head state, or `None` before the first slot.
    pub async fn don_state_at_head(&self) -> Result<Option<DonState>> {
        Ok(self
            .slot_state_at_head()
            .await?
            .map(|slot_state| slot_state.state))
    }

    /// Return the head state with its slot, or `None` before the first slot with an execution
    /// block.
    pub async fn slot_state_at_head(&self) -> Result<Option<SlotState>> {
        Self::read_slot_state_at_head(&self.pool).await
    }

    /// Read the head state over any executor (pool or transaction), so a
    /// caller can pin it to the same snapshot as other reads.
    async fn read_slot_state_at_head<'e, E: PgExecutor<'e>>(
        executor: E,
    ) -> Result<Option<SlotState>> {
        let row = sqlx::query(&format!(
            "SELECT {SLOT_STATE_COLUMNS} FROM slots \
             WHERE el_block_number IS NOT NULL ORDER BY slot DESC LIMIT 1"
        ))
        .fetch_optional(executor)
        .await?;
        row.as_ref().map(decode_slot_state_row).transpose()
    }

    /// Return the state derived at a committed execution block, or `None` when no committed
    /// slot holds that block.
    pub async fn slot_state_at_el_block(
        &self,
        block: BlockHashOrNumber,
    ) -> Result<Option<SlotState>> {
        let row = match block {
            BlockHashOrNumber::Number(number) => {
                let number = i64::try_from(number)
                    .with_context(|| format!("block number {number} exceeds the stored range"))?;
                sqlx::query(&format!(
                    "SELECT {SLOT_STATE_COLUMNS} FROM slots WHERE el_block_number = $1"
                ))
                .bind(number)
                .fetch_optional(&self.pool)
                .await?
            }
            BlockHashOrNumber::Hash(hash) => {
                sqlx::query(&format!(
                    "SELECT {SLOT_STATE_COLUMNS} FROM slots WHERE el_block_hash = $1"
                ))
                .bind(hash.as_slice().to_vec())
                .fetch_optional(&self.pool)
                .await?
            }
        };
        row.as_ref().map(decode_slot_state_row).transpose()
    }

    /// Return the beacon block root for a committed slot, `None` when it has none or is not
    /// committed.
    pub async fn slot_root(&self, slot: u32) -> Result<Option<B256>> {
        let root: Option<Option<Vec<u8>>> =
            sqlx::query_scalar("SELECT cl_block_root FROM slots WHERE slot = $1")
                .bind(slot as i32)
                .fetch_optional(&self.pool)
                .await?;
        root.flatten().as_deref().map(db_bytes_to_b256).transpose()
    }

    /// Return the nearest committed consensus block root strictly before `slot`, skipping empty slots.
    /// Fails when no preceding block has been committed.
    pub async fn latest_block_root_before(&self, slot: u32) -> Result<B256> {
        let root: Vec<u8> = sqlx::query_scalar(
            "SELECT cl_block_root FROM slots WHERE slot < $1 AND cl_block_root IS NOT NULL ORDER BY slot DESC LIMIT 1",
        )
        .bind(slot as i32)
        .fetch_one(&self.pool)
        .await?;
        db_bytes_to_b256(&root)
    }

    /// Commit one new slot as the new highest slot, writing
    /// its created-index rows in the same transaction so the index is always
    /// consistent with the committed state.
    ///
    /// Duplicate slot inserts are treated as logic bugs and must fail loudly.
    pub async fn commit_slot(
        &self,
        slot: u32,
        block_root: Option<B256>,
        derived: Option<&DerivedSlot>,
    ) -> Result<()> {
        let state = derived.map(|derived| &derived.state);
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            r#"
            INSERT INTO slots(
                slot,
                cl_block_root,
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
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
            "#,
        )
        .bind(slot as i32)
        .bind(block_root.map(|root| root.as_slice().to_vec()))
        .bind(state.map(|state| state.metadata.block.number as i64))
        .bind(state.map(|state| state.metadata.block.timestamp as i64))
        .bind(state.map(|state| state.metadata.block.hash.as_slice().to_vec()))
        .bind(state.map(|state| hash_to_db_bytes(state.roots.created)))
        .bind(state.map(|state| hash_to_db_bytes(state.roots.nullifiers)))
        .bind(state.map(|state| hash_to_db_bytes(state.roots.prior_state_history)))
        .bind(state.map(|state| hash_to_db_bytes(state.roots.next_state_history)))
        .bind(state.map(|state| hash_to_db_bytes(state.metadata.state_root)))
        .bind(state.map(|state| state.metadata.created_count as i64))
        .bind(state.map(|state| state.metadata.nullifier_count as i64))
        .bind(state.map(|state| state.metadata.state_root_count as i64))
        .execute(&mut *tx)
        .await?;

        for (commitment, index) in derived.iter().flat_map(|derived| &derived.created_added) {
            // `commitment` is the PRIMARY KEY; a conflict is a bug and fails loudly.
            sqlx::query(
                "INSERT INTO created_index(commitment, array_index, slot) VALUES ($1, $2, $3)",
            )
            .bind(hash_to_db_bytes(*commitment))
            .bind(*index)
            .bind(slot as i32)
            .execute(&mut *tx)
            .await
            .with_context(|| {
                format!("inserting created_index row for {commitment:#} at slot {slot}")
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

    /// Read the head slot state and the created indices for `commitments` from
    /// one `REPEATABLE READ` transaction, so the state roots and the index rows
    /// reflect a single consistent Postgres snapshot. Without this a concurrent
    /// `commit_slot`/`rollback_to_slot` could land between the two reads and make
    /// the index inconsistent with the state a query is answering against. The
    /// RocksDB array read needs no coordination: nodes are content-addressed and
    /// immutable by root, so reading at the pinned root is stable regardless.
    pub async fn slot_state_at_head_with_created_indices(
        &self,
        commitments: &[Hash],
    ) -> Result<(Option<SlotState>, HashMap<Hash, i64>)> {
        let mut tx = self
            .pool
            .begin_with("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .await?;
        let head = Self::read_slot_state_at_head(&mut *tx).await?;
        let indices = Self::read_created_indices(&mut *tx, commitments).await?;
        tx.commit().await?;
        Ok((head, indices))
    }

    /// Return the state roots derived at or above the given execution block number.
    pub async fn recent_state_roots(&self, min_block_number: u64) -> Result<Vec<(Hash, i64)>> {
        let rows = sqlx::query(
            r#"
            SELECT don_state_root, el_block_number
            FROM slots
            WHERE el_block_number >= $1
            ORDER BY el_block_number ASC, slot ASC
            "#,
        )
        .bind(min_block_number as i64)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|row| {
                let state_root: Vec<u8> = row.get("don_state_root");
                let block_number = row.get::<i64, _>("el_block_number");
                Ok((db_bytes_to_hash(&state_root)?, block_number))
            })
            .collect()
    }
}

fn db_bytes_to_b256(bytes: &[u8]) -> Result<B256> {
    B256::try_from(bytes).map_err(|_| anyhow!("expected 32 bytes, got {}", bytes.len()))
}

fn decode_slot_state_row(row: &PgRow) -> Result<SlotState> {
    let slot = row.get::<i32, _>("slot");
    Ok(SlotState {
        state: decode_state_row(row)
            .with_context(|| format!("slots row for slot {slot} had invalid state data"))?,
        slot: slot as u32,
    })
}

/// Decode the DON state from a row with an execution block, the block the state was derived at.
fn decode_state_row(row: &PgRow) -> Result<DonState> {
    let block = ExecutionBlock {
        number: row.get::<i64, _>("el_block_number") as u64,
        timestamp: row.get::<i64, _>("el_block_timestamp") as u64,
        hash: db_bytes_to_b256(&row.get::<Vec<u8>, _>("el_block_hash"))?,
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

    fn derived(state: DonState) -> DerivedSlot {
        DerivedSlot {
            state,
            created_added: HashMap::new(),
        }
    }

    fn unique_hash(n: i64) -> Hash {
        hash_values(&[Value::from(n)])
    }

    fn unique_execution_block(block_number: u64) -> ExecutionBlock {
        ExecutionBlock {
            number: block_number,
            timestamp: block_number * 1000,
            hash: B256::repeat_byte(block_number as u8),
        }
    }

    fn unique_state(block_number: u64, marker: i64) -> DonState {
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
                created_count: block_number,
                nullifier_count: block_number + 1,
                state_root_count: block_number + 2,
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
        let db_name = format!("state_log_db_test_{}_{}_{}", pid, nanos, ctr);
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

    async fn fresh_state_log() -> Result<(StateLogDb, String, String)> {
        let (db_name, db_url, admin_url) = test_urls();
        create_db(&db_name, &admin_url).await?;
        let state_log = StateLogDb::connect(&db_url, 1).await?;
        Ok((state_log, db_name, admin_url))
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
        let (state_log, db_name, admin_url) = fresh_state_log().await?;

        assert_eq!(state_log.last_processed_slot().await?, None);
        assert!(state_log.slot_state_at_head().await?.is_none());

        drop_db(&db_name, &admin_url).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local postgres"]
    async fn test_connect_rejects_changed_init_start_slot() -> Result<()> {
        let (db_name, db_url, admin_url) = test_urls();
        create_db(&db_name, &admin_url).await?;
        let state_log = StateLogDb::connect(&db_url, 5).await?;
        state_log.commit_slot(5, None, None).await?;

        assert!(StateLogDb::connect(&db_url, 5).await.is_ok());
        assert!(StateLogDb::connect(&db_url, 4).await.is_err());

        drop_db(&db_name, &admin_url).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local postgres"]
    async fn test_commit_slot_persists_state_and_latest_slot() -> Result<()> {
        let (state_log, db_name, admin_url) = fresh_state_log().await?;
        let state = unique_state(7, 100);

        state_log
            .commit_slot(10, None, Some(&derived(state)))
            .await?;

        let head = state_log
            .slot_state_at_head()
            .await?
            .expect("a slot is committed");
        assert_eq!(head.state, state);
        assert_eq!(head.slot, 10);

        drop_db(&db_name, &admin_url).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local postgres"]
    async fn test_head_state_comes_from_last_slot_with_execution_block() -> Result<()> {
        let (state_log, db_name, admin_url) = fresh_state_log().await?;
        let state = unique_state(7, 100);
        state_log
            .commit_slot(10, None, Some(&derived(state)))
            .await?;
        // Later slots without an execution block derive no state.
        state_log
            .commit_slot(11, Some(B256::repeat_byte(11)), None)
            .await?;
        state_log.commit_slot(12, None, None).await?;

        let head = state_log
            .slot_state_at_head()
            .await?
            .expect("a slot is committed");
        assert_eq!(head.state, state);
        assert_eq!(head.slot, 10);
        assert_eq!(state_log.last_processed_slot().await?, Some(12));

        drop_db(&db_name, &admin_url).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local postgres"]
    async fn test_commit_slot_duplicate_slot_fails() -> Result<()> {
        let (state_log, db_name, admin_url) = fresh_state_log().await?;
        let state1 = unique_state(7, 100);
        let state2 = unique_state(8, 200);

        state_log
            .commit_slot(10, None, Some(&derived(state1)))
            .await?;

        let err = state_log
            .commit_slot(10, None, Some(&derived(state2)))
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
        let (state_log, db_name, admin_url) = fresh_state_log().await?;
        let h1 = unique_state(5, 10);
        state_log.commit_slot(1, None, Some(&derived(h1))).await?;

        let h2 = unique_state(9, 20);
        state_log.commit_slot(2, None, Some(&derived(h2))).await?;

        let recent = state_log.recent_state_roots(6).await?;
        assert_eq!(recent, vec![(h2.metadata.state_root, 9)]);

        drop_db(&db_name, &admin_url).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local postgres"]
    async fn test_rollback_rewinds_latest_slot_and_state() -> Result<()> {
        let (state_log, db_name, admin_url) = fresh_state_log().await?;
        let h1 = unique_state(1, 10);
        state_log.commit_slot(1, None, Some(&derived(h1))).await?;

        let h2 = unique_state(2, 20);
        state_log.commit_slot(2, None, Some(&derived(h2))).await?;

        state_log.rollback_to_slot(1).await?;

        let head = state_log
            .slot_state_at_head()
            .await?
            .expect("a slot is committed");
        assert_eq!(head.state, h1);
        assert_eq!(head.slot, 1);

        drop_db(&db_name, &admin_url).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local postgres"]
    async fn test_created_index_commit_and_rollback() -> Result<()> {
        let (state_log, db_name, admin_url) = fresh_state_log().await?;
        let a = unique_hash(1);
        let b = unique_hash(2);

        let h1 = unique_state(1, 10);
        state_log
            .commit_slot(
                1,
                None,
                Some(&DerivedSlot {
                    state: h1,
                    created_added: HashMap::from([(a, 1)]),
                }),
            )
            .await?;
        let h2 = unique_state(2, 20);
        state_log
            .commit_slot(
                2,
                None,
                Some(&DerivedSlot {
                    state: h2,
                    created_added: HashMap::from([(b, 2)]),
                }),
            )
            .await?;

        let indices = state_log.created_indices(&[a, b]).await?;
        assert_eq!(indices.get(&a), Some(&1));
        assert_eq!(indices.get(&b), Some(&2));

        // Rolling back to slot 1 prunes slot 2's index row in the same
        // transaction as the slot delete.
        state_log.rollback_to_slot(1).await?;
        let indices = state_log.created_indices(&[a, b]).await?;
        assert_eq!(indices.get(&a), Some(&1));
        assert_eq!(indices.get(&b), None);

        // The atomic read returns the same index against the head state.
        let (head, indices) = state_log
            .slot_state_at_head_with_created_indices(&[a, b])
            .await?;
        assert_eq!(head.expect("a slot is committed").slot, 1);
        assert_eq!(indices.get(&a), Some(&1));
        assert_eq!(indices.get(&b), None);

        drop_db(&db_name, &admin_url).await?;
        Ok(())
    }
}
