# `dobjd`

The Digital Objects Network driver daemon. Wraps `Arc<driver::Driver>` behind an HTTP/SSE
API and an MCP server so that every client (terminal CLI, desktop app,
website, MCP-aware agents) talks to a single driver process per machine.

`dobjd` owns `~/.dobj/` exclusively — the RocksDB lock, the in-memory
catalog, and the broadcast event hub. The companion CLI
(`dobj`, in the [`cli/`](../../interfaces/cli) crate) and the React frontend in
[`gui/`](../../interfaces/gui) are thin clients over the API surface here.

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
  [`dobj-mcp`](../../interfaces/mcp) crate) sharing the same `Arc<Driver>` as the
  HTTP routes, so an MCP-driven action shows up in real time on every
  other connected client.

dobjd is API-only — the UI is served separately (Vite on `:1420` in
dev, Tauri's webview for the desktop app).

## HTTP API

All routes return JSON unless noted; errors come back as
`{"error": "..."}` with an appropriate status code.

| Method | Path                            | Driver call                                                  |
| ------ | ------------------------------- | ------------------------------------------------------------ |
| `GET`  | `/objects`                      | `sync_objects` (with fallback to `list_objects`)             |
| `GET`  | `/actions`                      | `list_actions`                                               |
| `GET`  | `/state-root`                   | `get_state_root`                                             |
| `GET`  | `/objects/dir`                  | `paths().objects_dir`                                        |
| `POST` | `/objects/import`               | `import_object` (body: `{ "dobj": "<json>" }`)               |
| `GET`  | `/objects/{file_name}`          | `read_object(&Path)` (basename in `~/.dobj/objects/`)        |
| `GET`  | `/classes`                      | `list_classes`                                               |
| `GET`  | `/classes/{name}`               | `get_class`                                                  |
| `GET`  | `/settings`                     | `load_settings`                                              |
| `PUT`  | `/settings`                     | `save_settings`                                              |
| `POST` | `/actions/run`                  | starts a run, returns `202 { runId, status }` (non-blocking) |
| `GET`  | `/actions/runs/{run_id}`        | run status + result/error + progress log (poll)              |
| `GET`  | `/actions/runs/{run_id}/events` | per-run SSE: replays buffered progress then tails live       |
| `GET`  | `/actions/{id}`                 | `get_action`                                                 |
| `GET`  | `/actions/{id}/feasibility`     | `check_action`                                               |
| `GET`  | `/events`                       | global broadcast hub stream (SSE)                            |

### Runs are non-blocking

`POST /actions/run` registers the run, kicks off a background worker, and
returns a `runId` immediately; the proof + commit pipeline runs on the worker.
The worker records the run's status, ordered progress log, and terminal
result/error in an in-memory registry. Follow a run either way:

- **Poll** `GET /actions/runs/{run_id}` for the current state (the
  disconnect-recovery path — a client that lost its connection re-reads the
  outcome here).
- **Stream** `GET /actions/runs/{run_id}/events`, which replays the buffered
  progress (honoring `Last-Event-ID` on reconnect) then tails live events
  until the run is terminal.

Each `POST` mints a fresh `runId`; clients don't choose it. Terminal runs are
retained for a short TTL then reaped; runs are in-memory only (on-chain state
and local `.dobj` files reconcile via sync regardless). The global `/events`
stream carries every run's progress (`type: run-action-progress`) for firehose
subscribers.

## MCP integration

The `mcp` module ([`src/mcp.rs`](src/mcp.rs)) is the glue between this crate
and [`mcp/`](../../interfaces/mcp). `DobjdOps` implements `DobjOps` against the same
`Arc<Driver>` and run registry the HTTP routes use, so `run_action` starts a
run (returning a `runId`) and `get_run` polls it — an MCP-driven action is the
same run object the desktop GUI / website / `dobj` CLI can follow, and its
progress fans out over the shared SSE hub in real time.

## Build and run

```bash
# from the workspace root
cargo run --release -p dobjd

# or via the just recipe (matches what `just dev` uses)
just dobjd

# different HTTP port; MCP binds to the adjacent port, 127.0.0.1:7728:
DOBJD_PORT=7727 cargo run --release -p dobjd
```

Released binaries are signed + notarized on macOS. The tarball is just
the three executables (`dobjd`, `dobj`, `dobj-mcp-proxy`).
Windows binaries are not codesigned yet — first run shows a SmartScreen warning.

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
[`cli/src/daemon.rs`](../../interfaces/cli/src/daemon.rs).

## Settings

Read on every `load_settings()` call from `~/.dobj/settings.json`:

```json
{
  "synchronizerApiUrl": "https://synchronizer.don.pateldhvani.com",
  "relayerApiUrl": "https://relayer.don.pateldhvani.com"
}
```

Compile-time defaults come from `DEFAULT_SYNCHRONIZER_API_URL` /
`DEFAULT_RELAYER_API_URL` env vars at build time (set by the release
workflow), overridden by anything in the settings file at runtime.
