use std::collections::HashSet;

use anyhow::{Context, Result};
use sqlx::{PgPool, Row};

#[derive(Debug)]
pub struct DerivedState {
    pub transactions: HashSet<String>,
    pub nullifiers: HashSet<String>,
}

pub struct Db {
    pool: PgPool,
}

impl Db {
    pub async fn connect(database_url: &str) -> Result<Self> {
        let pool = PgPool::connect(database_url).await?;
        Ok(Self { pool })
    }

    pub async fn init(&self) -> Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS sync_state (
                id SMALLINT PRIMARY KEY CHECK (id = 1),
                last_processed_slot BIGINT NOT NULL,
                last_processed_block_number BIGINT,
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS transactions (
                object_id TEXT PRIMARY KEY,
                first_seen_slot BIGINT NOT NULL,
                first_seen_block_number BIGINT,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS nullifiers (
                object_id TEXT PRIMARY KEY,
                first_seen_slot BIGINT NOT NULL,
                first_seen_block_number BIGINT,
                consumed_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn load_state(&self) -> Result<DerivedState> {
        let transaction_rows = sqlx::query("SELECT object_id FROM transactions")
            .fetch_all(&self.pool)
            .await?;
        let nullifier_rows = sqlx::query("SELECT object_id FROM nullifiers")
            .fetch_all(&self.pool)
            .await?;

        let transactions = transaction_rows
            .into_iter()
            .map(|row| row.get::<String, _>("object_id"))
            .collect();
        let nullifiers = nullifier_rows
            .into_iter()
            .map(|row| row.get::<String, _>("object_id"))
            .collect();

        Ok(DerivedState {
            transactions,
            nullifiers,
        })
    }

    pub async fn last_processed_slot(&self) -> Result<Option<u32>> {
        let row = sqlx::query("SELECT last_processed_slot FROM sync_state WHERE id = 1")
            .fetch_optional(&self.pool)
            .await?;

        let Some(row) = row else {
            return Ok(None);
        };

        let slot_i64: i64 = row.get("last_processed_slot");
        let slot =
            u32::try_from(slot_i64).context("Stored last_processed_slot does not fit in u32")?;
        Ok(Some(slot))
    }

    pub async fn mark_slot_processed(&self, slot: u32, block_number: Option<u32>) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO sync_state (id, last_processed_slot, last_processed_block_number, updated_at)
            VALUES (1, $1, $2, NOW())
            ON CONFLICT (id) DO UPDATE
            SET
                last_processed_slot = EXCLUDED.last_processed_slot,
                last_processed_block_number = COALESCE(EXCLUDED.last_processed_block_number, sync_state.last_processed_block_number),
                updated_at = NOW()
            "#,
        )
        .bind(i64::from(slot))
        .bind(block_number.map(i64::from))
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn persist_transaction(
        &self,
        object_id: &str,
        slot: u32,
        block_number: Option<u32>,
    ) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO transactions (object_id, first_seen_slot, first_seen_block_number)
            VALUES ($1, $2, $3)
            ON CONFLICT (object_id) DO NOTHING
            "#,
        )
        .bind(object_id)
        .bind(i64::from(slot))
        .bind(block_number.map(i64::from))
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
