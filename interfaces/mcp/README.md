# Digital Objects Network MCP Server

An MCP (Model Context Protocol) server that exposes dobj's digital object
operations to AI agents. Claude Code, Claude Desktop, Cursor, and any other
MCP-aware client can inspect objects, explore actions, read documentation, run
ZK-proof-based actions, and drive an optional terse command UX through this
server.

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

The server exposes three MCP surfaces -- **tools** (generic verbs), **prompts**
(an opt-in command UX), and **resources** (reference docs) -- and ships a
self-contained live **dashboard**.

### Crate structure

```
mcp/
  src/
    lib.rs          McpServer, McpConfig — HTTP embedding; writes the dashboard to ~/.dobj/dashboard on serve
    ops.rs          DobjOps trait — boundary between MCP and the host
    types.rs        MCP request/response types (JsonSchema-derived)
    server.rs       DobjMcpService — tool handlers + ServerHandler (tools, prompts, resources)
    prompts.rs      Built-in prompts: the start dispatcher, help, create-command, consult-docs, dashboard
    commands.rs     CommandStore — user-authored commands under ~/.dobj/commands/<name>/
    resources.rs    MCP resources (docs + podlang source files)
    mock.rs         MockDobjOps — realistic test fixtures
    bin/
      mock_server.rs   Standalone HTTP server with mock data (port 7718)
      mock_stdio.rs    Standalone stdio server with mock data
      proxy_stdio.rs   Stdio↔HTTP proxy (binary: dobj-mcp-proxy)
  docs/             Prompt bodies + reference docs (start.md, help.md, create-command.md, how-it-works.md, …)
  dashboard/
    index.html      Self-contained live dashboard (no build step)
```

The `mcp` crate has **no dependencies** on pod2, txlib, craft_sdk, dobjd,
or gui. The `DobjOps` trait is the integration boundary — the
production implementation lives in
[`dobjd/src/mcp.rs`](../../services/dobjd/src/mcp.rs); the test implementation is
`MockDobjOps`.

## Tools

| Tool                                                  | Description                                                     |
| ----------------------------------------------------- | --------------------------------------------------------------- |
| `list_objects`                                        | All objects with class, fields, and liveness                    |
| `list_actions`                                        | Available actions with input/output classes and cost            |
| `list_classes`                                        | Object classes with live counts and producing/consuming actions |
| `inspect_object` / `inspect_class` / `inspect_action` | Full detail on one object / class / action, with predicate      |
| `check_feasibility`                                   | Whether an action can run with current objects; missing inputs  |
| `run_action`                                          | Start an action; returns a `runId` immediately (non-blocking)   |
| `get_run`                                             | Poll a run's status, result/error, and progress log by `runId`  |
| `get_state_root`                                      | Current state root from the synchronizer                        |
| `import_object_file`                                  | Adopt an external `.dobj` from a local path into objects        |
| `read_settings` / `write_settings`                    | Synchronizer + relayer URLs                                     |
| `get_objects_dir`                                     | Path to `~/.dobj/objects/`                                      |
| `read_doc`                                            | Reference docs (see Resources)                                  |
| `define_command` / `delete_command` / `list_commands` | Manage user-authored commands                                   |
| `get_command`                                         | Load any command's full body (built-in or saved) to follow it   |

All tools return structured content (`structuredContent` + `outputSchema`)
for clients that support it, with a text fallback for older clients.

## Prompts — the command UX

The server exposes an opt-in, terse command interface as MCP prompts, so it
works in any MCP client with no skills, hooks, or client-specific config.

- `start` — enter a command session. Injects a dispatcher persona plus the live
  list of installed commands; from there, the user types a command's name to
  run it.
- `help` — the command menu.
- `create-command` — a guided interview that saves a new command.
- `consult-docs` — answer a question from the reference docs, quoted verbatim.
- `dashboard` — open or close the live dashboard (a pane in Claude Code,
  otherwise a URL). Pass `stop` to close.
- one dynamic prompt per user-authored command.

When the user types a command's name, the model calls `get_command(name)` to
load that command's full body — built-in or saved — then follows it. The
always-on server instructions point clients at this flow, so a command name
works whether typed inside a `start` session or invoked directly as the prompt
`/mcp__dobj__<name>`.

### User-authored commands

Saved commands live under `~/.dobj/commands/<name>/`, each a `README.md` (YAML
frontmatter `name` + `description`, then the instruction body the model follows)
plus any sibling scripts the command runs. The MCP server owns this directory;
the driver is not involved. `define_command` writes the README, `create-command`
is the guided authoring flow, and `list_commands` / `delete_command` manage them.
Each saved command is also surfaced as its own prompt.

## Resources

Reference docs, available both as MCP resources and through the `read_doc` tool:

| `read_doc` name     | URI                             | Contents                                                 |
| ------------------- | ------------------------------- | -------------------------------------------------------- |
| `podlang-reference` | `dobj://docs/podlang-reference` | Full podlang language reference                          |
| `object-lifecycle`  | `dobj://docs/object-lifecycle`  | Object creation / mutation / consumption walkthrough     |
| `how-it-works`      | `dobj://docs/how-it-works`      | Plugin-agnostic framing for working with Digital Objects |
| `command-examples`  | `dobj://docs/command-examples`  | Templates for `create-command` bodies                    |
| `txlib.podlang`     | `dobj://source/txlib.podlang`   | Core transaction-model predicates source                 |

`read_doc` additionally serves `generated.podlang` (rendered from the live
driver) and `time.podlang`.

## Dashboard

A self-contained live dashboard (`dashboard/index.html`, no build step). dobjd
writes it to `~/.dobj/dashboard/index.html` when the MCP server starts; the
`dashboard` prompt serves it — in Claude Code, a preview pane backed by a local static
server on `:7719`; otherwise it points the user at the file. The page polls the
REST API on `:7717` for objects, the synchronizer head, and an action-log SSE.

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

Tests run against `MockDobjOps` and cover tool handlers, structured output,
error cases, concurrent action dispatch, the prompt surface, and the command
store.

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

### Adding a tool

1. Add the method to `DobjOps` in `ops.rs`
2. Add the mock implementation in `mock.rs`
3. Add the tool handler in `server.rs` (use `#[tool(description = "...")]`)
4. Add request/response types to `types.rs` if needed
5. Add the matching method to `DobjdOps` in `dobjd/src/mcp.rs`
6. Update the tool count assertion in `tests::test_tool_router_lists_all_tools`

### Adding a built-in command

Add a `Builtin` entry in `prompts.rs` with a body under `docs/<name>.md`, and
add its name to the reserved list in `commands.rs`. It is then listed by `help`,
loadable via `get_command`, and available as the prompt `/mcp__dobj__<name>`.

```

```
