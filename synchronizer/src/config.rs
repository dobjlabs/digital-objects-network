use std::{net::SocketAddr, str::FromStr, time::Duration};

use alloy::primitives::Address;
use anyhow::Result;

const DEFAULT_ROCKSDB_PATH: &str = "data/synchronizer-rocksdb";
const DEFAULT_HTTP_BIND: &str = "127.0.0.1:3000";
const DEFAULT_SYNC_DELAY_MS: u64 = 333;
const DEFAULT_INITIAL_START_SLOT: u32 = 0;

pub struct AppConfig {
    pub rocksdb_path: String,
    pub http_bind: SocketAddr,
    pub sync_delay: Duration,
    pub initial_start_slot: u32,
    pub rpc_url: String,
    pub beacon_url: String,
    pub to_address: Address,
}

pub fn load_config() -> Result<AppConfig> {
    let rocksdb_path =
        dotenvy::var("ROCKSDB_PATH").unwrap_or_else(|_| DEFAULT_ROCKSDB_PATH.to_string());
    let http_bind = dotenvy::var("HTTP_BIND").unwrap_or_else(|_| DEFAULT_HTTP_BIND.to_string());
    let http_bind: SocketAddr = http_bind.parse()?;
    let sync_delay_ms = dotenvy::var("SYNC_DELAY_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_SYNC_DELAY_MS);
    let initial_start_slot = dotenvy::var("INITIAL_START_SLOT")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(DEFAULT_INITIAL_START_SLOT);

    let rpc_url: String = dotenvy::var("RPC_URL")?;
    let beacon_url: String = dotenvy::var("BEACON_URL")?;
    let to_address: Address = Address::from_str(&dotenvy::var("TO_ADDRESS")?)?;

    Ok(AppConfig {
        rocksdb_path,
        http_bind,
        sync_delay: Duration::from_millis(sync_delay_ms),
        initial_start_slot,
        rpc_url,
        beacon_url,
        to_address,
    })
}
