/// Stdio-to-HTTP proxy for the ZK-Craft MCP server.
///
/// Claude Desktop launches this as a child process. It reads JSON-RPC from
/// stdin, forwards to the Tauri app's streamable HTTP MCP endpoint, and
/// writes responses to stdout.
use std::io::{BufRead, Write};

fn main() -> anyhow::Result<()> {
    craft_mcp::logging::init_stderr();

    let url = parse_url_from_args();
    tracing::info!("ZK-Craft MCP proxy connecting to {url}");

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()?;
    let mut session_id: Option<String> = None;

    let stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout().lock();

    for line in stdin.lines() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Check if this is a request (has "id") or notification (no "id")
        let parsed: serde_json::Value = serde_json::from_str(line)?;
        let is_request = parsed.get("id").is_some();

        // Build the HTTP request
        let mut req = client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream");

        if let Some(sid) = &session_id {
            req = req.header("Mcp-Session-Id", sid);
        }

        let resp = req.body(line.to_string()).send().map_err(|e| {
            anyhow::anyhow!(
                "Failed to connect to MCP server at {url}: {e}. Is the Tauri app running?"
            )
        })?;

        // Capture session ID from response
        if let Some(sid) = resp.headers().get("mcp-session-id") {
            session_id = Some(sid.to_str().unwrap_or_default().to_string());
        }

        if !is_request {
            // Notification — no response expected on stdio
            continue;
        }

        // Parse SSE response to extract JSON-RPC messages
        let body = resp.text()?;
        for sse_line in body.lines() {
            if let Some(data) = sse_line.strip_prefix("data: ") {
                let data = data.trim();
                if data.is_empty() {
                    continue;
                }
                // Validate it's JSON before forwarding
                if data.starts_with('{') {
                    writeln!(stdout, "{data}")?;
                    stdout.flush()?;
                }
            }
        }
    }

    Ok(())
}

fn parse_url_from_args() -> String {
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--port" => {
                if let Some(port) = args.get(i + 1) {
                    return format!("http://127.0.0.1:{port}/mcp");
                }
                i += 2;
            }
            "--url" => {
                if let Some(url) = args.get(i + 1) {
                    return url.clone();
                }
                i += 2;
            }
            _ => i += 1,
        }
    }
    format!("http://127.0.0.1:{}/mcp", craft_mcp::DEFAULT_PORT)
}
