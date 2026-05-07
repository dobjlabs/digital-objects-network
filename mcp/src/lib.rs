pub mod logging;
pub mod mock;
pub mod ops;
pub mod resources;
pub mod server;
pub mod types;

/// Default port for the MCP server. Adjacent to dobjd's default HTTP API
/// port on 7717 so the two ports read as a pair in `lsof -i` / `ss`
/// output. dobjd derives custom MCP ports as `DOBJD_PORT + 1`.
pub const DEFAULT_PORT: u16 = 7718;

use std::sync::Arc;

use ops::CraftOps;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use server::CraftMcpService;
use tokio_util::sync::CancellationToken;

/// Configuration for the MCP server.
pub struct McpConfig {
    /// Cancellation token for graceful shutdown.
    pub cancellation_token: CancellationToken,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            cancellation_token: CancellationToken::new(),
        }
    }
}

/// Top-level MCP server handle.
///
/// Wraps `CraftOps` and provides an axum router that can be mounted
/// into any axum application or served standalone.
pub struct McpServer<T: CraftOps> {
    ops: Arc<T>,
    config: McpConfig,
}

impl<T: CraftOps> McpServer<T> {
    pub fn new(ops: T, config: McpConfig) -> Self {
        Self {
            ops: Arc::new(ops),
            config,
        }
    }

    /// Build an axum `Router` with the MCP service mounted at `/mcp`.
    pub fn router(self) -> axum::Router {
        let ops = self.ops;
        let ct = self.config.cancellation_token;

        let service = StreamableHttpService::new(
            move || Ok(CraftMcpService::new(ops.clone())),
            LocalSessionManager::default().into(),
            StreamableHttpServerConfig {
                cancellation_token: ct.child_token(),
                ..Default::default()
            },
        );

        axum::Router::new().nest_service("/mcp", service)
    }

    /// Serve the MCP server on the given TCP listener.
    /// Blocks until the cancellation token is cancelled or Ctrl+C.
    pub async fn serve(self, listener: tokio::net::TcpListener) -> anyhow::Result<()> {
        let ct = self.config.cancellation_token.clone();
        let router = self.router();

        axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                ct.cancelled().await;
            })
            .await?;

        Ok(())
    }
}
