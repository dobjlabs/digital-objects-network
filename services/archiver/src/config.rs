use std::{net::SocketAddr, str::FromStr};

use alloy::primitives::Address;
use anyhow::Result;

const DEFAULT_HTTP_BIND: &str = "127.0.0.1:3001";

#[derive(Debug)]
pub struct Config {
    pub blobs_path: String,
    pub http_bind: SocketAddr,
    pub init_start_slot: u32,
    pub rpc_url: String,
    pub beacon_url: String,
    // Only store blobs from transactions with this destination
    pub filter_address: Address,
}

pub fn load_config() -> Result<Config> {
    let _ = dotenvy::from_filename("services/archiver/.env");

    let http_bind = dotenvy::var("HTTP_BIND").unwrap_or_else(|_| DEFAULT_HTTP_BIND.to_string());
    let http_bind: SocketAddr = http_bind.parse()?;

    let blobs_path = dotenvy::var("BLOBS_PATH").unwrap();
    let init_start_slot = dotenvy::var("INIT_START_SLOT")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap();

    let rpc_url: String = dotenvy::var("RPC_URL")?;
    let beacon_url: String = dotenvy::var("BEACON_URL")?;
    let filter_address: Address = Address::from_str(&dotenvy::var("FILTER_ADDRESS")?)?;

    Ok(Config {
        blobs_path,
        http_bind,
        init_start_slot,
        rpc_url,
        beacon_url,
        filter_address,
    })
}
