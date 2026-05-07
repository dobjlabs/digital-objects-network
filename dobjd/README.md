# `dobjd`

The bitcraft driver daemon. Wraps `Arc<driver::Driver>` behind an HTTP/SSE
API and an MCP server so that every client (terminal CLI, desktop app,
website, MCP-aware agents) talks to a single driver process per machine.

`dobjd` owns `~/.dobj/` exclusively — the RocksDB lock, the in-memory
catalog, and the broadcast event hub. The companion CLI
(`dobj`, in the [`cli/`](../cli) crate) and the React frontend in
[`app-gui/`](../app-gui) are thin clients over the API surface here.

## What it does

```
        ┌─ desktop (Tauri webview shell) ─┐
        ├─ browser tab ───────────────────┤   HTTP/SSE
        ├─ `dobj` terminal CLI ───────────┼──────────►  dobjd  ──►  ~/.dobj/
        └─ MCP agents (Claude, Cursor) ───┘            (this crate)
```

Two concurrent listeners:

- **REST/SSE on `127.0.0.1:7717`** (configurable via `DOBJD_PORT`) —
  routes mirror driver capabilities and there's a single SSE event
  stream every client subscribes to.
- **MCP on `127.0.0.1:7718`** (`DOBJD_PORT + 1`) — streamable-HTTP MCP server (from the
  [`craft-mcp`](../mcp) crate) sharing the same `Arc<Driver>` as the
  HTTP routes, so an MCP-driven action shows up in real time on every
  other connected client.

dobjd is API-only — the UI is served separately (Vite on `:1420` in
dev, Tauri's webview for the desktop app).

## HTTP API

All routes return JSON unless noted; errors come back as
`{"error": "..."}` with an appropriate status code.

| Method | Path | Driver call |
|---|---|---|
| `GET` | `/inventory` | `sync_inventory` (with fallback to `list_objects`) + `list_actions` |
| `GET` | `/state-root` | `get_state_root` |
| `GET` | `/objects/dir` | `paths().objects_dir` |
| `POST` | `/objects/parse` | `parse_object_record_bytes` (multipart `file` upload, no disk write) |
| `GET` | `/objects/{id}` | `read_object(ObjectSelector::ObjectId)` |
| `GET` | `/classes` | `list_classes` |
| `GET` | `/classes/{name}` | `get_class` |
| `GET` | `/settings` | `load_settings` |
| `PUT` | `/settings` | `save_settings` |
| `POST` | `/actions/run` | `execute_with_reporter` |
| `GET` | `/actions/{id}/feasibility` | `check_action` |
| `GET` | `/events` | broadcast hub stream (SSE) |

The `/events` payload is a JSON object with a `type` discriminator.
Variants: `run-action-progress`.

## MCP integration

The `mcp` module ([`src/mcp.rs`](src/mcp.rs)) is the glue between this
crate and [`mcp/`](../mcp). Its `DobjdCraftOps` overrides
`run_action_with_progress` to fan execution events to **both** the SSE
event hub (so the desktop GUI / website / `dobj watch` see progress) and
the MCP-supplied `ProgressReporter` callback (so the agent that
triggered the action gets `notifications/progress`).

## Build and run

```bash
# from the workspace root
cargo run --release -p dobjd

# or via the just recipe (matches what `just dev` uses)
just dobjd

# different HTTP port; MCP binds to the adjacent port, 127.0.0.1:7728:
DOBJD_PORT=7727 cargo run --release -p dobjd
```

Released binaries are signed + notarized on macOS and bundle
`libscip*.dylib` plus the GCC runtime libs (`libgfortran`,
`libquadmath`, `libgcc_s`) in a hidden `.libs/` subdir next to the
executable. The RPATH baked in by [build.rs](build.rs) resolves them at
runtime with no env vars. See
[`.github/workflows/release.yml`](../.github/workflows/release.yml) for
the packaging pipeline.

## Lifecycle

`dobjd` is a long-running daemon. It doesn't background itself — that's
the CLI's job:

```bash
dobj start     # spawns dobjd detached (setsid + pidfile)
dobj status    # pid + HTTP healthcheck
dobj logs -f   # tail ~/.dobj/dobjd.log
dobj stop      # SIGTERM, SIGKILL fallback
```

The CLI's daemon-management implementation is in
[`cli/src/daemon.rs`](../cli/src/daemon.rs).

## Settings

Read on every `load_settings()` call from `~/.dobj/settings.json`:

```json
{
  "synchronizerApiUrl": "http://18.119.100.201:3000",
  "relayerApiUrl": "http://18.119.100.201:3200"
}
```

Compile-time defaults come from `DEFAULT_SYNCHRONIZER_API_URL` /
`DEFAULT_RELAYER_API_URL` env vars at build time (set by the release
workflow), overridden by anything in the settings file at runtime.
