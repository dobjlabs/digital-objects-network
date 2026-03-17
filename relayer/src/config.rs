use std::{net::SocketAddr, str::FromStr};

use alloy::primitives::Address;
use anyhow::{Context, Result};

const DEFAULT_HTTP_BIND: &str = "127.0.0.1:3200";
const DEFAULT_DB_URL: &str = "postgres://postgres@localhost:5432/relayer";
const DEFAULT_MAX_ATTEMPTS: u32 = 8;
const DEFAULT_RETRY_INITIAL_SECS: u64 = 4;
const DEFAULT_RETRY_MAX_SECS: u64 = 300;
const DEFAULT_RECEIPT_POLL_SECS: u64 = 6;
const DEFAULT_WORKER_IDLE_SLEEP_MS: u64 = 1000;

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub bind: SocketAddr,
    pub db_url: String,
    pub rpc_url: String,
    pub to_address: Address,
    pub private_key: String,
    pub max_attempts: u32,
    pub retry_initial_secs: u64,
    pub retry_max_secs: u64,
    pub receipt_poll_secs: u64,
    pub receipt_timeout_secs: Option<u64>,
    pub worker_idle_sleep_ms: u64,
    pub max_fee_per_blob_gas: Option<u128>,
}

pub fn load_config() -> Result<AppConfig> {
    let _ = dotenvy::from_filename("relayer/.env");
    let _ = dotenvy::dotenv();

    let bind = dotenvy::var("HTTP_BIND").unwrap_or_else(|_| DEFAULT_HTTP_BIND.to_string());
    let bind: SocketAddr = bind.parse().context("invalid HTTP_BIND")?;

    let db_url = dotenvy::var("DB_URL").unwrap_or_else(|_| DEFAULT_DB_URL.to_string());
    let rpc_url = dotenvy::var("RPC_URL").context("RPC_URL is required")?;
    let to_address =
        Address::from_str(&dotenvy::var("TO_ADDRESS").context("TO_ADDRESS is required")?)
            .context("invalid TO_ADDRESS")?;
    let private_key = dotenvy::var("PRIVATE_KEY").context("PRIVATE_KEY is required")?;

    let max_attempts = dotenvy::var("MAX_ATTEMPTS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_MAX_ATTEMPTS)
        .max(1);

    let retry_initial_secs = dotenvy::var("RETRY_INITIAL_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_RETRY_INITIAL_SECS)
        .max(1);

    let retry_max_secs = dotenvy::var("RETRY_MAX_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_RETRY_MAX_SECS)
        .max(retry_initial_secs);

    let receipt_poll_secs = dotenvy::var("RECEIPT_POLL_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_RECEIPT_POLL_SECS)
        .max(1);

    let receipt_timeout_secs = dotenvy::var("RECEIPT_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0);

    let worker_idle_sleep_ms = dotenvy::var("WORKER_IDLE_SLEEP_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_WORKER_IDLE_SLEEP_MS)
        .max(100);

    let max_fee_per_blob_gas = dotenvy::var("MAX_FEE_PER_BLOB_GAS")
        .ok()
        .and_then(|v| v.parse::<u128>().ok());

    Ok(AppConfig {
        bind,
        db_url,
        rpc_url,
        to_address,
        private_key,
        max_attempts,
        retry_initial_secs,
        retry_max_secs,
        receipt_poll_secs,
        receipt_timeout_secs,
        worker_idle_sleep_ms,
        max_fee_per_blob_gas,
    })
}
