use craft_mcp::mock::MockCraftOps;
use craft_mcp::server::CraftMcpService;
use rmcp::ServiceExt;
use rmcp::transport::io::stdio;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    craft_mcp::logging::init_stderr();

    tracing::info!("ZK-Craft MCP mock server starting (stdio transport)");

    let service = CraftMcpService::new(std::sync::Arc::new(MockCraftOps::new()));
    let running = service.serve(stdio()).await?;
    running.waiting().await?;

    Ok(())
}
