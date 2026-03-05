use alloy::primitives::B256;
use anyhow::{anyhow, Context, Result};
use pod2::middleware::Hash;
use sqlx::{postgres::PgPoolOptions, Executor, PgPool, Row};
use url::Url;

use crate::app_db::{db_bytes_to_hash, hash_to_db_bytes};

#[derive(Debug, Clone, Copy)]
pub struct SyncProgress {
    pub last_processed_slot: u32,
    pub last_processed_block_number: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct SlotJournal {
    pub slot: u32,
    pub tx_hashes: Vec<Hash>,
    pub nullifiers: Vec<Hash>,
    pub gsr_block_numbers: Vec<u32>,
    pub gsr_hashes: Vec<Hash>,
}

#[derive(Debug, Clone)]
pub struct PendingRecovery {
    pub slot: u32,
    pub block_number: Option<u32>,
    pub journal: SlotJournal,
}

#[derive(Clone)]
pub struct SyncDb {
    pool: PgPool,
}

impl SyncDb {
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

    async fn bootstrap(&self) -> Result<()> {
        let statements = [
            r#"
            CREATE TABLE IF NOT EXISTS sync_cursor (
                id SMALLINT PRIMARY KEY CHECK (id = 1),
                last_processed_slot INTEGER NULL,
                last_processed_block_number INTEGER NULL,
                next_slot INTEGER NOT NULL,
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

    pub async fn initialize_cursor_if_missing(
        &self,
        head_slot: u32,
        initial_start: Option<u32>,
    ) -> Result<u32> {
        let start_slot = initial_start.unwrap_or(head_slot);
        sqlx::query(
            r#"
            INSERT INTO sync_cursor (id, last_processed_slot, last_processed_block_number, next_slot)
            VALUES (1, NULL, NULL, $1)
            ON CONFLICT (id) DO NOTHING
            "#,
        )
        .bind(start_slot as i32)
        .execute(&self.pool)
        .await?;

        let row = sqlx::query("SELECT next_slot FROM sync_cursor WHERE id = 1")
            .fetch_one(&self.pool)
            .await?;
        let next_slot: i32 = row.get("next_slot");
        Ok(next_slot as u32)
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
            INSERT INTO slot_apply_journal(slot, block_root, tx_hashes, nullifiers, gsr_block_numbers, gsr_hashes, kv_applied)
            VALUES ($1, $2, $3, $4, $5, $6, false)
            ON CONFLICT (slot) DO UPDATE
            SET block_root = EXCLUDED.block_root,
                tx_hashes = EXCLUDED.tx_hashes,
                nullifiers = EXCLUDED.nullifiers,
                gsr_block_numbers = EXCLUDED.gsr_block_numbers,
                gsr_hashes = EXCLUDED.gsr_hashes,
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
            INSERT INTO sync_cursor (id, last_processed_slot, last_processed_block_number, next_slot)
            VALUES (1, $1, $2, $3)
            ON CONFLICT (id) DO UPDATE
            SET last_processed_slot = EXCLUDED.last_processed_slot,
                last_processed_block_number = EXCLUDED.last_processed_block_number,
                next_slot = EXCLUDED.next_slot,
                updated_at = now()
            "#,
        )
        .bind(slot as i32)
        .bind(block_number.map(|v| v as i32))
        .bind((slot + 1) as i32)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }

    pub async fn pending_recoveries(&self) -> Result<Vec<PendingRecovery>> {
        let rows = sqlx::query(
            r#"
            SELECT c.slot,
                   c.execution_block_number,
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
            recoveries.push(PendingRecovery {
                slot: row.get::<i32, _>("slot") as u32,
                block_number: row
                    .get::<Option<i32>, _>("execution_block_number")
                    .map(|v| v as u32),
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
            sqlx::query("DELETE FROM canonical_slots WHERE slot > $1")
                .bind(keep_slot as i32)
                .execute(&mut *tx)
                .await?;

            sqlx::query(
                r#"
                INSERT INTO sync_cursor (id, last_processed_slot, last_processed_block_number, next_slot)
                VALUES (
                    1,
                    $1,
                    (SELECT execution_block_number FROM canonical_slots WHERE slot = $1),
                    $2
                )
                ON CONFLICT (id) DO UPDATE
                SET last_processed_slot = EXCLUDED.last_processed_slot,
                    last_processed_block_number = EXCLUDED.last_processed_block_number,
                    next_slot = EXCLUDED.next_slot,
                    updated_at = now()
                "#,
            )
            .bind(keep_slot as i32)
            .bind((keep_slot + 1) as i32)
            .execute(&mut *tx)
            .await?;
        } else {
            sqlx::query("DELETE FROM canonical_slots")
                .execute(&mut *tx)
                .await?;

            sqlx::query(
                r#"
                INSERT INTO sync_cursor (id, last_processed_slot, last_processed_block_number, next_slot)
                VALUES (1, NULL, NULL, 0)
                ON CONFLICT (id) DO UPDATE
                SET last_processed_slot = NULL,
                    last_processed_block_number = NULL,
                    next_slot = 0,
                    updated_at = now()
                "#,
            )
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(journals)
    }
}

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
