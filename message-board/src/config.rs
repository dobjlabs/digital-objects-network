use std::net::SocketAddr;

use anyhow::{Context, Result};

const DEFAULT_BIND: &str = "127.0.0.1:3100";
const DEFAULT_CORS_ORIGIN: &str = "*";
const DEFAULT_MAX_PAGE_SIZE: u32 = 50;

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub bind: SocketAddr,
    pub database_url: String,
    pub cors_origin: String,
    pub max_page_size: u32,
}

pub fn load_config() -> Result<AppConfig> {
    // Support running from repo root (`cargo run -p message-board`) and from
    // inside `message-board/` directly.
    let _ = dotenvy::from_filename("message-board/.env");
    let _ = dotenvy::dotenv();

    let bind: SocketAddr = dotenvy::var("MESSAGE_BOARD_BIND")
        .unwrap_or_else(|_| DEFAULT_BIND.to_string())
        .parse()
        .context("invalid MESSAGE_BOARD_BIND")?;

    let database_url = dotenvy::var("MESSAGE_BOARD_DATABASE_URL")
        .context("MESSAGE_BOARD_DATABASE_URL is required")?;

    let cors_origin = dotenvy::var("MESSAGE_BOARD_CORS_ORIGIN")
        .unwrap_or_else(|_| DEFAULT_CORS_ORIGIN.to_string());

    let max_page_size = dotenvy::var("MESSAGE_BOARD_MAX_PAGE_SIZE")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(DEFAULT_MAX_PAGE_SIZE)
        .max(1);

    Ok(AppConfig {
        bind,
        database_url,
        cors_origin,
        max_page_size,
    })
}
