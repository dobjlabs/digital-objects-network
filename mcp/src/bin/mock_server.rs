use craft_mcp::mock::MockCraftOps;
use craft_mcp::{DEFAULT_PORT, McpConfig, McpServer};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    craft_mcp::logging::init();

    let bind_address = format!("127.0.0.1:{DEFAULT_PORT}");
    let ct = tokio_util::sync::CancellationToken::new();
    let config = McpConfig {
        cancellation_token: ct.clone(),
    };
    let server = McpServer::new(MockCraftOps::new(), config);
    let listener = tokio::net::TcpListener::bind(&bind_address).await?;

    tracing::info!("bitcraft MCP mock server listening on http://{bind_address}/mcp");

    let ct2 = ct.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.unwrap();
        tracing::info!("Shutting down...");
        ct2.cancel();
    });

    server.serve(listener).await
}
