use dobj_mcp::mock::MockDobjOps;
use dobj_mcp::server::DobjMcpService;
use rmcp::ServiceExt;
use rmcp::transport::io::stdio;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dobj_mcp::logging::init_stderr();

    tracing::info!("Digital Objects MCP mock server starting (stdio transport)");

    let service = DobjMcpService::new(std::sync::Arc::new(MockDobjOps::new()));
    let running = service.serve(stdio()).await?;
    running.waiting().await?;

    Ok(())
}
