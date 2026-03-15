use rmcp::ServiceExt;
use rmcp::transport::io::stdio;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};
use zk_craft_mcp::mock::MockCraftOps;
use zk_craft_mcp::server::CraftMcpService;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Log to stderr so stdout stays clean for JSON-RPC
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .init();

    tracing::info!("ZK-Craft MCP mock server starting (stdio transport)");

    let service = CraftMcpService::new(std::sync::Arc::new(MockCraftOps::new()));
    let running = service.serve(stdio()).await?;
    running.waiting().await?;

    Ok(())
}
