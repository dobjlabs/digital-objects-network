use std::{net::SocketAddr, str::FromStr, time::Duration};

use alloy::primitives::Address;
use anyhow::Result;
use tracing::warn;

// const DEFAULT_APP_STATE_DB_PATH: &str = "data/synchronizer-db";
// const DEFAULT_SYNC_METADATA_DB_URL: &str = "postgres://postgres@localhost:5432/synchronizer";
const DEFAULT_HTTP_BIND: &str = "127.0.0.1:3001";
// const DEFAULT_RPC_RETRIES: u32 = 6;
// const DEFAULT_RPC_RETRY_MS: u64 = 1_000;

/// Known RPC rate limit. Used to derive safe defaults for batch size and sync delay.
const RPC_RATE_LIMIT_PER_SEC: u64 = 15;

/// Each slot in a batch makes ~2 RPC calls (header + block). Use half the rate limit to leave
/// headroom for retries and the single-slot path.
const fn default_catchup_batch_size() -> usize {
    (RPC_RATE_LIMIT_PER_SEC as usize) / 2
}

#[derive(Debug)]
pub struct AppConfig {
    // pub app_state_db_path: String,
    // pub sync_metadata_db_url: String,
    // pub rpc_retries: u32,
    // pub rpc_retry_delay: Duration,
    // pub catchup_batch_size: usize,
    pub blobs_path: String,
    pub http_bind: SocketAddr,
    pub request_rate: u64,
    pub init_slot: u32,
    pub rpc_url: String,
    pub beacon_url: String,
    pub filter_address: Address,
}

pub fn load_config() -> Result<AppConfig> {
    let _ = dotenvy::from_filename("synchronizer/.env");

    // let app_state_db_path =
    //     dotenvy::var("APP_STATE_DB_PATH").unwrap_or_else(|_| DEFAULT_APP_STATE_DB_PATH.to_string());
    // let sync_metadata_db_url = dotenvy::var("SYNC_METADATA_DB_URL")
    //     .unwrap_or_else(|_| DEFAULT_SYNC_METADATA_DB_URL.to_string());
    let http_bind = dotenvy::var("HTTP_BIND").unwrap_or_else(|_| DEFAULT_HTTP_BIND.to_string());
    let http_bind: SocketAddr = http_bind.parse()?;

    let request_rate = dotenvy::var("REQUEST_RATE")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(RPC_RATE_LIMIT_PER_SEC);

    // let catchup_batch_size = match dotenvy::var("CATCHUP_BATCH_SIZE") {
    //     Ok(v) => {
    //         let size: usize = v.parse()?;
    //         anyhow::ensure!(size > 0, "CATCHUP_BATCH_SIZE must be greater than 0");
    //         warn!(
    //             catchup_batch_size = size,
    //             rpc_rate_limit = RPC_RATE_LIMIT_PER_SEC,
    //             default_size = default_catchup_batch_size(),
    //             "CATCHUP_BATCH_SIZE overridden via env; ensure this respects the RPC rate limit"
    //         );
    //         size
    //     }
    //     Err(_) => default_catchup_batch_size(),
    // };

    // let rpc_retries = dotenvy::var("RPC_RETRIES")
    //     .ok()
    //     .and_then(|v| v.parse::<u32>().ok())
    //     .unwrap_or(DEFAULT_RPC_RETRIES);
    // let rpc_retry_ms = dotenvy::var("RPC_RETRY_MS")
    //     .ok()
    //     .and_then(|v| v.parse::<u64>().ok())
    //     .unwrap_or(DEFAULT_RPC_RETRY_MS);
    let blobs_path = dotenvy::var("BLOBS_PATH").unwrap();
    let init_slot = dotenvy::var("INIT_SLOT")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap();

    let rpc_url: String = dotenvy::var("RPC_URL")?;
    let beacon_url: String = dotenvy::var("BEACON_URL")?;
    let filter_address: Address = Address::from_str(&dotenvy::var("FILTER_ADDRESS")?)?;

    Ok(AppConfig {
        // app_state_db_path,
        // sync_metadata_db_url,
        // rpc_retries,
        // rpc_retry_delay: Duration::from_millis(rpc_retry_ms),
        // catchup_batch_size,
        blobs_path,
        http_bind,
        request_rate,
        init_slot,
        rpc_url,
        beacon_url,
        filter_address,
    })
}
