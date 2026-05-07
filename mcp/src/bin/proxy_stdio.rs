/// Stdio-to-HTTP proxy for the bitcraft MCP server.
///
/// Claude Desktop only speaks stdio MCP. This binary reads JSON-RPC from
/// stdin, forwards it to dobjd's streamable HTTP MCP endpoint
/// (`http://127.0.0.1:7718/mcp` by default), and writes the response back
/// to stdout.
///
/// Requests are dispatched concurrently so that long-running tool calls
/// (e.g. proof generation) do not block other requests.
use std::io::{BufRead, Write};
use std::sync::Arc;

use tokio::sync::{Notify, RwLock, mpsc};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    craft_mcp::logging::init_stderr();

    let url = parse_url_from_args();
    tracing::info!("ZK-Craft MCP proxy connecting to {url}");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()?;

    let session_id: Arc<RwLock<Option<String>>> = Arc::new(RwLock::new(None));
    let session_ready = Arc::new(Notify::new());

    // Channel for serialised stdout writes (avoids interleaving).
    let (stdout_tx, mut stdout_rx) = mpsc::unbounded_channel::<String>();

    // Stdout writer task.
    tokio::spawn(async move {
        while let Some(line) = stdout_rx.recv().await {
            let mut stdout = std::io::stdout().lock();
            let _ = writeln!(stdout, "{line}");
            let _ = stdout.flush();
        }
    });

    // Read stdin on a dedicated thread (blocking I/O).
    let (stdin_tx, mut stdin_rx) = mpsc::unbounded_channel::<String>();
    std::thread::spawn(move || {
        let stdin = std::io::stdin().lock();
        for line in stdin.lines() {
            let Ok(line) = line else { break };
            let line = line.trim().to_string();
            if line.is_empty() {
                continue;
            }
            if stdin_tx.send(line).is_err() {
                break;
            }
        }
    });

    // Dispatch loop.
    let mut first = true;
    while let Some(line) = stdin_rx.recv().await {
        if first {
            // The first request (typically `initialize`) must complete before
            // concurrent requests can be dispatched, because we need the
            // session ID from the response.
            first = false;
            handle_request(&client, &url, &line, &session_id, &stdout_tx).await;
            session_ready.notify_waiters();
        } else {
            let client = client.clone();
            let url = url.clone();
            let session_id = session_id.clone();
            let session_ready = session_ready.clone();
            let stdout_tx = stdout_tx.clone();
            tokio::spawn(async move {
                // Wait until the session has been established.
                //
                // Register the Notified future *before* checking session_id,
                // so we can't miss a notify_waiters() that races between the
                // check and the await. Tokio's Notify only wakes waiters that
                // were already registered when notify_waiters() fires.
                let waiter = session_ready.notified();
                if session_id.read().await.is_none() {
                    waiter.await;
                }
                handle_request(&client, &url, &line, &session_id, &stdout_tx).await;
            });
        }
    }

    Ok(())
}

async fn handle_request(
    client: &reqwest::Client,
    url: &str,
    line: &str,
    session_id: &Arc<RwLock<Option<String>>>,
    stdout_tx: &mpsc::UnboundedSender<String>,
) {
    let parsed: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("invalid JSON from stdin: {e}");
            return;
        }
    };
    let is_request = parsed.get("id").is_some();

    let mut req = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream");

    if let Some(sid) = session_id.read().await.as_deref() {
        req = req.header("Mcp-Session-Id", sid);
    }

    let resp = match req.body(line.to_string()).send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(
                "Failed to connect to MCP server at {url}: {e}. Is dobjd running? Try `dobj status`."
            );
            return;
        }
    };

    // Capture session ID from response.
    if let Some(sid) = resp.headers().get("mcp-session-id") {
        if let Ok(sid) = sid.to_str() {
            *session_id.write().await = Some(sid.to_string());
        }
    }

    if !is_request {
        // Notification — no response expected on stdio.
        return;
    }

    // Parse SSE response to extract JSON-RPC messages.
    let body = match resp.text().await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("failed to read response body: {e}");
            return;
        }
    };
    for sse_line in body.lines() {
        if let Some(data) = sse_line.strip_prefix("data: ") {
            let data = data.trim();
            if data.is_empty() {
                continue;
            }
            if data.starts_with('{') {
                let _ = stdout_tx.send(data.to_string());
            }
        }
    }
}

/// Parse `--url <url>` or `--port <num>` from argv. The two are mutually
/// exclusive; `--url` wins if both are provided. Missing values exit non-zero
/// rather than silently falling back, so a typo can't quietly point the
/// proxy at the wrong endpoint.
fn parse_url_from_args() -> String {
    let mut args = std::env::args().skip(1);
    let mut url: Option<String> = None;
    let mut port: Option<String> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--url" => {
                url = Some(args.next().unwrap_or_else(|| die("--url requires a value")));
            }
            "--port" => {
                port = Some(
                    args.next()
                        .unwrap_or_else(|| die("--port requires a value")),
                );
            }
            other => die(&format!("unknown argument: {other}")),
        }
    }
    if let Some(url) = url {
        return url;
    }
    let port = port.unwrap_or_else(|| craft_mcp::DEFAULT_PORT.to_string());
    format!("http://127.0.0.1:{port}/mcp")
}

fn die(msg: &str) -> ! {
    eprintln!("bitcraft-mcp-proxy: {msg}");
    eprintln!("usage: bitcraft-mcp-proxy [--url <url> | --port <num>]");
    std::process::exit(2);
}
