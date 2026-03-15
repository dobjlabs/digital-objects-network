use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};
use zk_craft_mcp::{McpConfig, McpServer};
use zk_craft_mcp::mock::MockCraftOps;

const BIND_ADDRESS: &str = "127.0.0.1:3001";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let ct = tokio_util::sync::CancellationToken::new();
    let config = McpConfig {
        cancellation_token: ct.clone(),
    };
    let server = McpServer::new(MockCraftOps::new(), config);
    let listener = tokio::net::TcpListener::bind(BIND_ADDRESS).await?;

    tracing::info!("ZK-Craft MCP mock server listening on http://{BIND_ADDRESS}/mcp");

    let ct2 = ct.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.unwrap();
        tracing::info!("Shutting down...");
        ct2.cancel();
    });

    server.serve(listener).await
}
