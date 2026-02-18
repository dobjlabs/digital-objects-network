use std::collections::HashSet;

use anyhow::{Context, Result};
use sqlx::{PgPool, Row};
use url::Url;

#[derive(Debug)]
pub struct DerivedState {
    pub transactions: HashSet<String>,
    pub nullifiers: HashSet<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct SyncProgress {
    pub last_processed_slot: u32,
    pub last_processed_block_number: Option<u32>,
}

pub struct Db {
    pool: PgPool,
}

pub async fn ensure_database_exists(database_url: &str) -> Result<()> {
    let parsed = Url::parse(database_url).context("Invalid DATABASE_URL")?;
    let db_name = parsed.path().trim_start_matches('/').to_string();
    if db_name.is_empty() {
        anyhow::bail!("DATABASE_URL must include a database name in the path");
    }

    let mut admin_url = parsed;
    admin_url.set_path("/postgres");
    admin_url.set_query(None);
    admin_url.set_fragment(None);

    let admin_pool = PgPool::connect(admin_url.as_str())
        .await
        .context("Failed to connect to postgres admin database")?;

    let exists = sqlx::query("SELECT 1 FROM pg_database WHERE datname = $1")
        .bind(&db_name)
        .fetch_optional(&admin_pool)
        .await?
        .is_some();

    if !exists {
        let escaped_db_name = db_name.replace('"', "\"\"");
        let create_query = format!("CREATE DATABASE \"{}\"", escaped_db_name);
        if let Err(err) = sqlx::query(&create_query).execute(&admin_pool).await {
            match &err {
                sqlx::Error::Database(db_err) if db_err.code().as_deref() == Some("42P04") => {}
                _ => return Err(err.into()),
            }
        }
    }

    Ok(())
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
        Ok(self
            .last_progress()
            .await?
            .map(|progress| progress.last_processed_slot))
    }

    pub async fn last_progress(&self) -> Result<Option<SyncProgress>> {
        let row = sqlx::query(
            "SELECT last_processed_slot, last_processed_block_number FROM sync_state WHERE id = 1",
        )
        .fetch_optional(&self.pool)
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };

        let slot_i64: i64 = row.get("last_processed_slot");
        let slot =
            u32::try_from(slot_i64).context("Stored last_processed_slot does not fit u32")?;
        let block_i64: Option<i64> = row.get("last_processed_block_number");
        let block = match block_i64 {
            Some(value) => Some(
                u32::try_from(value)
                    .context("Stored last_processed_block_number does not fit u32")?,
            ),
            None => None,
        };

        Ok(Some(SyncProgress {
            last_processed_slot: slot,
            last_processed_block_number: block,
        }))
    }

    pub async fn mark_slot_processed(&self, slot: u32, block_number: Option<u32>) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO sync_state (id, last_processed_slot, last_processed_block_number, updated_at)
            VALUES (1, $1, $2, NOW())
            ON CONFLICT (id) DO UPDATE
            SET
                last_processed_slot = EXCLUDED.last_processed_slot,
                last_processed_block_number = EXCLUDED.last_processed_block_number,
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

    pub async fn persist_nullifier(
        &self,
        object_id: &str,
        slot: u32,
        block_number: Option<u32>,
    ) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO nullifiers (object_id, first_seen_slot, first_seen_block_number)
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
