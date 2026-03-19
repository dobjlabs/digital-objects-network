# ZK-Craft MCP Server

An MCP (Model Context Protocol) server that exposes ZK-Craft's digital object operations to AI agents. Claude Desktop or Claude Code can inspect inventory, explore crafting actions, read documentation, and execute ZK proof-based crafting operations through this server.

## Architecture

```
Claude Desktop <--stdio--> craft-mcp-proxy <--HTTP--> Tauri app (MCP + GUI)
```

The Tauri app embeds a streamable HTTP MCP server on port 3001. A thin stdio proxy binary bridges Claude Desktop (which expects stdio transport) to the app's HTTP endpoint. The GUI and MCP server share in-process state — when Claude runs an action, the GUI shows real-time progress.

### Crate structure

```
mcp/
  src/
    lib.rs          McpServer, McpConfig — HTTP embedding interface
    ops.rs          CraftOps trait — boundary between MCP and the host app
    types.rs        MCP request/response types (simple, LLM-friendly, with JsonSchema)
    server.rs       CraftMcpService — rmcp tool handlers and ServerHandler impl
    mock.rs         MockCraftOps — realistic test fixtures
    resources.rs    MCP resources (docs + podlang source files)
    bin/
      mock_server.rs   Standalone HTTP server with mock data (port 3001)
      mock_stdio.rs    Standalone stdio server with mock data
      proxy_stdio.rs   Stdio-to-HTTP proxy for Claude Desktop
  docs/
    podlang-reference.md   Full podlang language reference
    object-lifecycle.md    Digital Object lifecycle walkthrough
```

The `mcp` crate has **no dependencies** on pod2, txlib, craft_sdk, or app-gui. The `CraftOps` trait is the integration boundary — the real implementation lives in `app-gui/src-tauri/src/mcp.rs`.

## Tools

| Tool | Description |
|------|-------------|
| `list_inventory` | All objects with types, fields, liveness status |
| `list_actions` | Available crafting actions with input/output classes and CPU cost |
| `list_classes` | All object classes with live counts and producing/consuming actions |
| `get_state_root` | Current global state root from the synchronizer |
| `inspect_object` | Full object detail: fields, class, liveness, predicate source |
| `inspect_class` | Class predicate definition and related actions |
| `run_action` | Execute a crafting action (blocks for proof generation) |
| `check_feasibility` | Check if an action can run with current inventory |
| `read_doc` | Read reference docs (podlang ref, object lifecycle, predicate sources, generated podlang) |

All tools return structured content (`structuredContent` + `outputSchema`) for clients that support it, with a text fallback for older clients.

## Setup with Claude Desktop

### 1. Build the proxy

```sh
cd zk-craft
cargo build -p craft-mcp --bin craft-mcp-proxy --features proxy --release
```

### 2. Add to Claude Desktop config

Open Claude Desktop settings and edit the MCP server configuration (`claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "zk-craft": {
      "command": "/absolute/path/to/zk-craft/target/release/craft-mcp-proxy",
      "args": ["--port", "3001"]
    }
  }
}
```

### 3. Start the Tauri app

The proxy connects to the app's MCP endpoint. The app (plus the synchronizer and relayer it depends on) must be running first:

```sh
cd zk-craft
just dev
```

This starts the synchronizer, relayer, and GUI together. The MCP server starts automatically on `http://127.0.0.1:3001/mcp`.

### 4. Restart Claude Desktop

Claude Desktop reads the config on startup. After adding the server config and starting the app, restart Claude Desktop. You should see "zk-craft" listed as a connected MCP server.

## Setup with Claude Code

The proxy works with Claude Code the same way as Claude Desktop. With the app running:

```sh
claude mcp add zk-craft /absolute/path/to/zk-craft/target/release/craft-mcp-proxy -- --port 3001
```

Or add to `.claude/settings.json` manually:

```json
{
  "mcpServers": {
    "zk-craft": {
      "command": "/absolute/path/to/zk-craft/target/release/craft-mcp-proxy",
      "args": ["--port", "3001"]
    }
  }
}
```

### Mock mode (no app required)

For development and testing without the full app stack, use the stdio mock server:

```sh
claude mcp add zk-craft-mock /absolute/path/to/zk-craft/target/release/craft-mcp-stdio
```

This serves mock data — useful for testing MCP tools or developing new ones.

## Development

### Running tests

```sh
cargo test -p craft-mcp --release
```

Tests run against `MockCraftOps` and cover tool handlers, structured output, error cases, and server startup.

### Mock servers

**HTTP mock** (for testing the proxy or direct HTTP clients):

```sh
cargo run -p craft-mcp --bin craft-mcp-mock --release
# Listens on http://127.0.0.1:3001/mcp
```

**Stdio mock** (for testing with Claude Desktop/Code without the app):

```sh
cargo run -p craft-mcp --bin craft-mcp-stdio --release
```

### Proxy

```sh
cargo run -p craft-mcp --bin craft-mcp-proxy --features proxy --release -- --port 3001
```

The proxy accepts `--port <PORT>` or `--url <URL>` to configure the upstream endpoint. Default: `http://127.0.0.1:3001/mcp`.

### Adding tools

1. Add the method to `CraftOps` in `ops.rs`
2. Add the mock implementation in `mock.rs`
3. Add the tool handler in `server.rs` (use `#[tool(description = "...")]`)
4. Add request/response types to `types.rs` if needed
5. Update the tool count assertion in the test
