pub mod commands;
pub mod logging;
pub mod mock;
pub mod ops;
pub mod prompts;
pub mod resources;
pub mod server;
pub mod types;

/// Default port for the MCP server. Adjacent to dobjd's default HTTP API
/// port on 7717 so the two ports read as a pair in `lsof -i` / `ss`
/// output. dobjd derives custom MCP ports as `DOBJD_PORT + 1`.
pub const DEFAULT_PORT: u16 = 7718;

/// The bundled live dashboard. A single self-contained page that polls the
/// daemon's REST API for objects, the synchronizer head, and an action-log SSE.
/// On startup it is written to `~/.dobj/view/index.html` so a static file
/// server (launched by the `view` command) can serve it -- it ships with the
/// MCP server, independent of the React GUI.
const DASHBOARD_HTML: &str = include_str!("../dashboard/index.html");

use std::sync::Arc;

use ops::DobjOps;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use server::DobjMcpService;
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
/// Wraps `DobjOps` and provides an axum router that can be mounted
/// into any axum application or served standalone.
pub struct McpServer<T: DobjOps> {
    ops: Arc<T>,
    config: McpConfig,
}

impl<T: DobjOps> McpServer<T> {
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
            move || Ok(DobjMcpService::new(ops.clone())),
            LocalSessionManager::default().into(),
            StreamableHttpServerConfig::default().with_cancellation_token(ct.child_token()),
        );

        axum::Router::new().nest_service("/mcp", service)
    }

    /// Serve the MCP server on the given TCP listener.
    /// Blocks until the cancellation token is cancelled or Ctrl+C.
    pub async fn serve(self, listener: tokio::net::TcpListener) -> anyhow::Result<()> {
        // Write the bundled dashboard to `~/.dobj/view/` so the static file
        // server launched by the `view` command can serve it; best-effort.
        if let Ok(objects_dir) = self.ops.get_objects_dir() {
            let mut dir = std::path::PathBuf::from(objects_dir);
            dir.pop();
            dir.push("view");
            if std::fs::create_dir_all(&dir).is_ok() {
                let _ = std::fs::write(dir.join("index.html"), DASHBOARD_HTML);
            }
        }

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
