//! MCP (Model Context Protocol) operations backed by the same `Arc<Driver>`
//! and broadcast hub used by the HTTP routes, plus the runtime that starts
//! and stops the embedded MCP server when the `mcpEnabled` setting changes.
//!
//! Most `DobjdOps` methods are thin passes through the driver -- the MCP
//! types are aliases for the same `wire-types` definitions the HTTP routes
//! use, so there's nothing to convert.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use dobj_mcp::CancellationToken;
use dobj_mcp::ops::DobjOps;
use dobj_mcp::types as mcp;
use tokio::task::JoinHandle;

use crate::events::EventTx;
use crate::runs::RunRegistry;

/// How long a stop waits for graceful shutdown (in-flight responses
/// flushing, sessions closing) before the serve task is aborted outright.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// Owns the embedded MCP server's lifecycle. When enabled, the listener is
/// bound and served; when disabled, the port is not bound at all. [`apply`]
/// is the single reconcile point for every settings write path (HTTP PUT,
/// MCP `write_settings`, startup).
///
/// [`apply`]: McpRuntime::apply
pub struct McpRuntime {
    driver: Arc<::driver::Driver>,
    events: EventTx,
    runs: RunRegistry,
    addr: String,
    dobj_port: u16,
    running: tokio::sync::Mutex<Option<RunningServer>>,
    prebound: tokio::sync::Mutex<Option<tokio::net::TcpListener>>,
}

struct RunningServer {
    cancel: CancellationToken,
    task: JoinHandle<()>,
}

impl McpRuntime {
    pub fn new(
        driver: Arc<::driver::Driver>,
        events: EventTx,
        runs: RunRegistry,
        addr: String,
        dobj_port: u16,
    ) -> Self {
        Self {
            driver,
            events,
            runs,
            addr,
            dobj_port,
            running: tokio::sync::Mutex::new(None),
            prebound: tokio::sync::Mutex::new(None),
        }
    }

    /// Bind the listener up front without serving it, so a taken port fails
    /// startup fast -- ahead of the slow circuit warm-up rather than after.
    /// The next [`apply(true)`] serves this pre-bound listener instead of
    /// binding a fresh one, so the first MCP action arrives on warm circuits.
    ///
    /// [`apply(true)`]: McpRuntime::apply
    pub async fn prebind(&self, enabled: bool) -> Result<()> {
        if !enabled {
            return Ok(());
        }
        let listener = tokio::net::TcpListener::bind(&self.addr)
            .await
            .with_context(|| format!("binding MCP listener on {}", self.addr))?;
        *self.prebound.lock().await = Some(listener);
        Ok(())
    }

    /// Reconcile the running server with the desired setting. Idempotent;
    /// concurrent calls serialize on the internal lock. A failed enable
    /// (e.g. the port is taken) returns the error to the caller and leaves
    /// the server stopped.
    pub async fn apply(self: &Arc<Self>, enabled: bool) -> Result<()> {
        let mut running = self.running.lock().await;
        if enabled {
            if running.is_some() {
                return Ok(());
            }
            // Serve the listener `prebind` bound at startup if there is one;
            // a runtime enable (no pre-bind) binds fresh here instead.
            let listener = match self.prebound.lock().await.take() {
                Some(listener) => listener,
                None => tokio::net::TcpListener::bind(&self.addr)
                    .await
                    .with_context(|| format!("binding MCP listener on {}", self.addr))?,
            };
            let ops = DobjdOps::new(
                self.driver.clone(),
                self.events.clone(),
                self.runs.clone(),
                self.clone(),
            );
            let cancel = CancellationToken::new();
            let config = dobj_mcp::McpConfig {
                cancellation_token: cancel.clone(),
                dobj_port: self.dobj_port,
            };
            let server = dobj_mcp::McpServer::new(ops, config);
            let task = tokio::spawn(async move {
                if let Err(err) = server.serve(listener).await {
                    // A crashed (not cancelled) MCP server leaves the daemon
                    // half-running; refuse to limp along.
                    tracing::error!("MCP server crashed: {err:#}");
                    std::process::exit(1);
                }
            });
            tracing::info!("MCP server listening on http://{}/mcp", self.addr);
            *running = Some(RunningServer { cancel, task });
        } else if let Some(server) = running.take() {
            // Hold the lock across shutdown deliberately: the serve task keeps
            // the listener bound until its future drops, so releasing here
            // would let a concurrent `apply(true)` try to rebind a port that
            // is still in use. The wait is bounded by SHUTDOWN_GRACE and is
            // near-instant once cancellation closes any open sessions.
            server.cancel.cancel();
            let mut task = server.task;
            if tokio::time::timeout(SHUTDOWN_GRACE, &mut task)
                .await
                .is_err()
            {
                // Dropping the serve future closes the listener and every
                // connection a lingering session was holding open.
                task.abort();
                let _ = task.await;
            }
            tracing::info!("MCP server stopped");
        }
        Ok(())
    }
}

pub struct DobjdOps {
    driver: Arc<::driver::Driver>,
    events: EventTx,
    runs: RunRegistry,
    mcp_runtime: Arc<McpRuntime>,
}

impl DobjdOps {
    pub fn new(
        driver: Arc<::driver::Driver>,
        events: EventTx,
        runs: RunRegistry,
        mcp_runtime: Arc<McpRuntime>,
    ) -> Self {
        Self {
            driver,
            events,
            runs,
            mcp_runtime,
        }
    }
}

impl DobjOps for DobjdOps {
    fn list_objects(&self) -> anyhow::Result<Vec<mcp::ObjectSummary>> {
        // The driver folds each object's class metadata (emoji, description)
        // into the summary, so clients need no `/classes` round-trip.
        self.driver.sync_objects(None)
    }

    fn list_actions(&self) -> anyhow::Result<Vec<mcp::ActionSummary>> {
        self.driver.list_actions(None)
    }

    fn list_classes(&self) -> anyhow::Result<Vec<mcp::ClassSummary>> {
        self.driver.list_classes()
    }

    fn get_state_root(&self) -> anyhow::Result<String> {
        Ok(payload::encode_hash_hex(&self.driver.get_state_root()?))
    }

    fn inspect_object(&self, file_name: &str) -> anyhow::Result<mcp::ObjectSummary> {
        self.driver.read_object(std::path::Path::new(file_name))
    }

    fn inspect_class(&self, class: &mcp::QualifiedName) -> anyhow::Result<mcp::ClassSummary> {
        self.driver.get_class(class)
    }

    fn inspect_action(&self, action: &mcp::QualifiedName) -> anyhow::Result<mcp::ActionSummary> {
        self.driver.get_action(action)
    }

    fn run_action(&self, input: mcp::RunActionInput) -> anyhow::Result<mcp::RunAccepted> {
        // Pass strings through verbatim — the driver extracts basenames via
        // `Path::file_name`, so an absolute path or a bare basename resolve to
        // the same managed file.
        let input_objects: Vec<String> = input
            .input_object_paths
            .iter()
            .map(|path| path.trim().to_string())
            .collect();

        // The run shares the same registry as the HTTP routes, so the GUI and
        // an agent can both follow the daemon-assigned run id.
        Ok(crate::runs::spawn_run(
            &self.runs,
            self.driver.clone(),
            self.events.clone(),
            input.action,
            input_objects,
        ))
    }

    fn get_run(&self, run_id: &str) -> anyhow::Result<mcp::RunState> {
        self.runs
            .get(run_id)
            .map(|entry| entry.snapshot())
            .ok_or_else(|| anyhow::anyhow!("unknown run: {run_id}"))
    }

    fn check_feasibility(
        &self,
        action: &mcp::QualifiedName,
    ) -> anyhow::Result<mcp::FeasibilityReport> {
        self.driver.check_action(action)
    }

    fn import_object_file(&self, path: &str) -> anyhow::Result<mcp::ObjectSummary> {
        // MCP runs in-process with the driver on the user's machine, so it
        // reads the file here and hands the contents to the same validation
        // core the HTTP route uses. Arbitrary path is fine: the agent already
        // has filesystem access, and import only ingests (never discloses).
        let contents = std::fs::read_to_string(path)
            .map_err(|err| anyhow::anyhow!("could not read .dobj file at {path}: {err}"))?;
        self.driver.import_object(&contents)
    }

    fn read_settings(&self) -> anyhow::Result<mcp::DriverSettings> {
        self.driver.load_settings()
    }

    fn write_settings(
        &self,
        patch: mcp::DriverSettingsPatch,
    ) -> anyhow::Result<mcp::DriverSettings> {
        // Merge onto current so an omitted field keeps its value; in
        // particular, a patch without `mcpEnabled` must not stop the server
        // the calling agent is connected through.
        let mut merged = self.driver.load_settings()?;
        patch.apply_to(&mut merged);
        let saved = self.driver.save_settings(&merged)?;
        // Reconcile asynchronously: this is a sync trait method, and an agent
        // disabling MCP over MCP needs this response to flush before the
        // graceful shutdown tears the transport down.
        let runtime = self.mcp_runtime.clone();
        let enabled = saved.mcp_enabled;
        tokio::spawn(async move {
            if let Err(err) = runtime.apply(enabled).await {
                tracing::error!("failed to apply MCP setting: {err:#}");
            }
        });
        Ok(saved)
    }

    fn get_objects_dir(&self) -> anyhow::Result<String> {
        Ok(self
            .driver
            .paths()
            .objects_dir
            .to_string_lossy()
            .to_string())
    }

    fn generated_podlang(&self) -> Option<String> {
        self.driver.generated_podlang()
    }
}
