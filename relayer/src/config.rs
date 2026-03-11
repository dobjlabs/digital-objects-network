use std::{net::SocketAddr, str::FromStr};

use alloy::primitives::Address;
use anyhow::{Context, Result};

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

    let bind = dotenvy::var("RELAYER_BIND").context("RELAYER_BIND is required")?;
    let bind: SocketAddr = bind.parse().context("invalid RELAYER_BIND")?;

    let db_url = dotenvy::var("RELAYER_DB_URL").context("RELAYER_DB_URL is required")?;
    let rpc_url = dotenvy::var("RELAYER_RPC_URL").context("RELAYER_RPC_URL is required")?;
    let to_address = Address::from_str(
        &dotenvy::var("RELAYER_TO_ADDRESS").context("RELAYER_TO_ADDRESS is required")?,
    )
    .context("invalid RELAYER_TO_ADDRESS")?;
    let private_key =
        dotenvy::var("RELAYER_PRIVATE_KEY").context("RELAYER_PRIVATE_KEY is required")?;

    let max_attempts = dotenvy::var("RELAYER_MAX_ATTEMPTS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_MAX_ATTEMPTS)
        .max(1);

    let retry_initial_secs = dotenvy::var("RELAYER_RETRY_INITIAL_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_RETRY_INITIAL_SECS)
        .max(1);

    let retry_max_secs = dotenvy::var("RELAYER_RETRY_MAX_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_RETRY_MAX_SECS)
        .max(retry_initial_secs);

    let receipt_poll_secs = dotenvy::var("RELAYER_RECEIPT_POLL_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_RECEIPT_POLL_SECS)
        .max(1);

    let receipt_timeout_secs = dotenvy::var("RELAYER_RECEIPT_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0);

    let worker_idle_sleep_ms = dotenvy::var("RELAYER_WORKER_IDLE_SLEEP_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_WORKER_IDLE_SLEEP_MS)
        .max(100);

    let max_fee_per_blob_gas = dotenvy::var("RELAYER_MAX_FEE_PER_BLOB_GAS")
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
