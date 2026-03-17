use std::{net::SocketAddr, str::FromStr, time::Duration};

use alloy::primitives::Address;
use anyhow::Result;

const DEFAULT_APP_STATE_DB_PATH: &str = "data/synchronizer-db";
const DEFAULT_SYNC_METADATA_DB_URL: &str = "postgres://postgres@localhost:5432/synchronizer";
const DEFAULT_HTTP_BIND: &str = "127.0.0.1:3000";
const DEFAULT_SYNC_DELAY_MS: u64 = 333;

#[derive(Debug)]
pub struct AppConfig {
    pub app_state_db_path: String,
    pub sync_metadata_db_url: String,
    pub http_bind: SocketAddr,
    pub cors_allowed_origins: Vec<String>,
    pub sync_delay: Duration,
    pub initial_start_slot: Option<u32>,
    pub rpc_url: String,
    pub beacon_url: String,
    pub to_address: Address,
}

pub fn load_config() -> Result<AppConfig> {
    let _ = dotenvy::from_filename("synchronizer/.env");

    let app_state_db_path =
        dotenvy::var("APP_STATE_DB_PATH").unwrap_or_else(|_| DEFAULT_APP_STATE_DB_PATH.to_string());
    let sync_metadata_db_url = dotenvy::var("SYNC_METADATA_DB_URL")
        .unwrap_or_else(|_| DEFAULT_SYNC_METADATA_DB_URL.to_string());
    let http_bind = dotenvy::var("HTTP_BIND").unwrap_or_else(|_| DEFAULT_HTTP_BIND.to_string());
    let http_bind: SocketAddr = http_bind.parse()?;
    let cors_allowed_origins = dotenvy::var("CORS_ALLOWED_ORIGINS")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect();
    let sync_delay_ms = dotenvy::var("SYNC_DELAY_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_SYNC_DELAY_MS);
    let initial_start_slot = dotenvy::var("INITIAL_START_SLOT")
        .ok()
        .and_then(|v| v.parse::<u32>().ok());

    let rpc_url: String = dotenvy::var("RPC_URL")?;
    let beacon_url: String = dotenvy::var("BEACON_URL")?;
    let to_address: Address = Address::from_str(&dotenvy::var("TO_ADDRESS")?)?;

    Ok(AppConfig {
        app_state_db_path,
        sync_metadata_db_url,
        http_bind,
        cors_allowed_origins,
        sync_delay: Duration::from_millis(sync_delay_ms),
        initial_start_slot,
        rpc_url,
        beacon_url,
        to_address,
    })
}
