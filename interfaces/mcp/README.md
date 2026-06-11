# Digital Objects Network MCP Server

An MCP (Model Context Protocol) server that exposes dobj's digital
object operations to AI agents. Claude Code, Claude Desktop, Cursor, and any
other MCP-aware client can inspect objects, explore actions, read
documentation, and execute ZK-proof-based actions through this server.

## Architecture

```
Claude Code, Cursor, …       ──HTTP──┐
Claude Desktop ──stdio── dobj-mcp-proxy ──HTTP──┤
                                                   ▼
                                                 dobjd
                                          (one driver process,
                                            shared with the
                                            desktop / web GUIs
                                            and the dobj CLI)
```

The MCP server is hosted by `dobjd` on `http://127.0.0.1:7718/mcp` alongside
the REST API on `:7717`. Both share the same `Arc<Driver>` and the same
broadcast hub, so an action kicked off via MCP shows real-time progress in
the desktop window or browser tab.

Clients that speak streamable HTTP MCP (Claude Code, Cursor, Continue, …)
connect to dobjd directly. Agents that only speak stdio (e.g. Claude Desktop)
launch `dobj-mcp-proxy` as a child process; the proxy bridges their
stdin/stdout to dobjd's HTTP endpoint.

### Crate structure

```
mcp/
  src/
    lib.rs          McpServer, McpConfig — HTTP embedding interface
    ops.rs          DobjOps trait — boundary between MCP and the host
    types.rs        MCP request/response types (JsonSchema-derived)
    server.rs       DobjMcpService — rmcp tool handlers + ServerHandler
    mock.rs         MockDobjOps — realistic test fixtures
    resources.rs    MCP resources (docs + podlang source files)
    bin/
      mock_server.rs   Standalone HTTP server with mock data (port 7718)
      mock_stdio.rs    Standalone stdio server with mock data
      proxy_stdio.rs   Stdio↔HTTP proxy (binary: dobj-mcp-proxy)
  docs/
    podlang-reference.md   Full podlang language reference
    object-lifecycle.md    Digital Object lifecycle walkthrough
```

The `mcp` crate has **no dependencies** on pod2, txlib, craft_sdk, dobjd,
or gui. The `DobjOps` trait is the integration boundary — the
production implementation lives in
[`dobjd/src/mcp.rs`](../../services/dobjd/src/mcp.rs); the test implementation is
`MockDobjOps`.

## Tools

| Tool                               | Description                                                    |
| ---------------------------------- | -------------------------------------------------------------- |
| `list_objects`                     | All objects with types, fields, liveness status                |
| `list_actions`                     | Available actions with input/output classes                    |
| `list_classes`                     | All object classes with live counts and related actions        |
| `get_state_root`                   | Current state root from the synchronizer                       |
| `inspect_object`                   | Full object detail: fields, class, liveness, predicate         |
| `import_object_file`               | Adopt an external `.dobj` from a local path into objects       |
| `inspect_class`                    | Class predicate definition and related actions                 |
| `run_action`                       | Start an action; returns a `runId` immediately (non-blocking)  |
| `get_run`                          | Poll a run's status, result/error, and progress log by `runId` |
| `check_feasibility`                | Whether an action can run with current objects                 |
| `read_settings` / `write_settings` | Synchronizer + relayer URLs and the `mcpEnabled` toggle (partial writes merge) |
| `get_objects_dir`                  | Path to `~/.dobj/objects/`                                     |
| `read_doc`                         | Reference docs (podlang, object-lifecycle, generated podlang)  |

All tools return structured content (`structuredContent` + `outputSchema`)
for clients that support it, with a text fallback for older clients.

## Setup

### Streamable HTTP clients (Claude Code, Cursor, Continue, …)

Make sure dobjd is running, then point the client at
`http://127.0.0.1:7718/mcp`:

```sh
claude mcp add --transport http dobj http://127.0.0.1:7718/mcp
```

This is what [SKILL.md](../../SKILL.md) automates as part of end-user install.

### Stdio-only agents (e.g. Claude Desktop)

Stdio-only agents launch `dobj-mcp-proxy` as a child process. The proxy
connects to dobjd's HTTP MCP endpoint over loopback. Edit
`claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "dobj": {
      "command": "/Users/<you>/.dobj/bin/dobj-mcp-proxy",
      "args": ["--port", "7718"]
    }
  }
}
```

The proxy accepts `--port <PORT>` or `--url <URL>`. Default upstream:
`http://127.0.0.1:7718/mcp`.

The release tarball ships the proxy binary at `~/.dobj/bin/dobj-mcp-proxy`
alongside `dobjd` and `dobj`. For a from-source build:

```sh
cargo build -p dobj-mcp --bin dobj-mcp-proxy --features proxy --release
```

## Development

### Running tests

```sh
cargo test -p dobj-mcp --release
```

Tests run against `MockDobjOps` and cover tool handlers, structured
output, error cases, and concurrent action dispatch.

### Mock servers (no dobjd required)

**HTTP mock** — for poking the wire format directly or testing the proxy
without dobjd:

```sh
cargo run -p dobj-mcp --bin dobj-mcp-mock --release
# Listens on http://127.0.0.1:7718/mcp
```

**Stdio mock** — for wiring Claude Desktop/Code straight to fixture data:

```sh
cargo run -p dobj-mcp --bin dobj-mcp-stdio --release
```

### Adding tools

1. Add the method to `DobjOps` in `ops.rs`
2. Add the mock implementation in `mock.rs`
3. Add the tool handler in `server.rs` (use `#[tool(description = "...")]`)
4. Add request/response types to `types.rs` if needed
5. Add the matching method to `DobjdOps` in `dobjd/src/mcp.rs`
6. Update the tool count assertion in `tests::test_tool_router_lists_all_tools`
