use anyhow::Result;
use sqlx::postgres::{PgConnectOptions, PgConnection, PgPoolOptions};
use sqlx::Connection;
use std::str::FromStr;
use tracing::info;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

mod api;
mod config;
mod db;
mod types;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer().without_time().with_target(false).compact())
        .with(EnvFilter::from_default_env())
        .init();

    let cfg = config::load_config()?;
    ensure_database_exists(&cfg.database_url).await?;

    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&cfg.database_url)
        .await?;

    db::run_migrations(&pool).await?;

    let state = api::AppState {
        pool,
        max_page_size: cfg.max_page_size,
    };

    let app = api::build_router(state, &cfg)?;
    let listener = tokio::net::TcpListener::bind(cfg.bind).await?;
    info!(bind = %cfg.bind, "message-board server listening");

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await?;

    Ok(())
}

async fn ensure_database_exists(database_url: &str) -> Result<()> {
    let options = PgConnectOptions::from_str(database_url)?;
    let database_name = options
        .get_database()
        .map(str::to_string)
        .filter(|name| !name.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("database name is missing in MESSAGE_BOARD_DATABASE_URL"))?;

    let admin_options = options.clone().database("postgres");
    let mut conn = PgConnection::connect_with(&admin_options).await?;

    let exists: Option<i32> = sqlx::query_scalar("SELECT 1 FROM pg_database WHERE datname = $1")
        .bind(&database_name)
        .fetch_optional(&mut conn)
        .await?;

    if exists.is_none() {
        let escaped = database_name.replace('\"', "\"\"");
        let create_stmt = format!("CREATE DATABASE \"{escaped}\"");
        sqlx::query(&create_stmt).execute(&mut conn).await?;
        info!(database = %database_name, "Created missing database");
    }

    Ok(())
}
