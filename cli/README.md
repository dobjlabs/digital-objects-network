# `cli` (binary: `dobj`)

Terminal client for [`dobjd`](../dobjd). Thin HTTP wrapper around the
daemon's REST + SSE API plus a small set of daemon-lifecycle commands
that handle detaching, pidfile management, and log tailing so the user
never types `nohup`.

The crate name is `cli`; the binary is `dobj` — short, easy to type, and
matches the `~/.dobj/` directory it talks to.

## Usage

```
dobj <command> [args...]   [--url <URL>]   [--json]
```

Two flavors of commands.

### API wrappers — every driver capability through one HTTP/SSE endpoint

| Command                                         | Hits                                                                       |
| ----------------------------------------------- | -------------------------------------------------------------------------- |
| `inventory`                                     | `GET /inventory`                                                           |
| `actions`                                       | `GET /actions`                                                             |
| `classes`                                       | `GET /classes`                                                             |
| `inspect-object <file_name>`                    | `GET /objects/{file_name}`                                                 |
| `inspect-class <plugin::class>`                 | `GET /classes/{name}`                                                      |
| `inspect-action <plugin::action>`               | `GET /actions/{id}`                                                        |
| `feasibility <plugin::action>`                  | `GET /actions/{id}/feasibility`                                            |
| `state-root`                                    | `GET /state-root`                                                          |
| `objects-dir`                                   | `GET /objects/dir`                                                         |
| `settings get`                                  | `GET /settings`                                                            |
| `settings set --synchronizer URL --relayer URL` | `PUT /settings` (omitted flags left unchanged)                             |
| `run <action> [paths...]`                       | `POST /actions/run` + filters `/events` for matching `run-action-progress` |
| `events`                                        | `GET /events` SSE — prints every event as JSON lines                       |

Each command renders human-friendly output by default. Pass `--json`
for the raw payload (suitable for `jq`).

### Daemon lifecycle — operates on local files, not HTTP

| Command            | What it does                                                                                       |
| ------------------ | -------------------------------------------------------------------------------------------------- |
| `start`            | spawns dobjd as a detached child (`setsid` on Unix), writes `~/.dobj/dobjd.pid`, polls until ready |
| `stop`             | reads the pidfile, sends `SIGTERM`, waits up to 10s, escalates to `SIGKILL` if needed              |
| `status`           | prints whether the pid is alive and HTTP responds; surfaces stale pidfiles                         |
| `logs [-f] [-n N]` | shows the last `N` lines (default 100) of `~/.dobj/dobjd.log`, optionally following                |

Lifecycle commands are intentionally CLI-only — daemon supervision is
a CLI-shaped concern by definition. Other clients (the desktop app, an
MCP agent) just assume `dobjd` is running and connect to it.

## Configuration

| Flag          | Env var     | Default                 | Notes                                                                                                                                               |
| ------------- | ----------- | ----------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------- |
| `--url <URL>` | `DOBJD_URL` | `http://127.0.0.1:7717` | dobjd's HTTP base URL. `start` parses this URL's port and launches the local daemon with `DOBJD_PORT=<port>`; dobjd then hosts MCP on `<port + 1>`. |
| `--json`      | —           | off                     | Machine-readable output where applicable.                                                                                                           |

The daemon-lifecycle commands also resolve the `dobjd` binary in this
order:

1. `$DOBJD_BIN` (explicit override)
2. `~/.dobj/bin/dobjd` (where SKILL.md installs it)
3. `dobjd` next to the running `dobj` (works for `cargo install`)
4. Bare `dobjd` in `$PATH`

## Build and run

```bash
# build the binary
cargo build --release -p cli

# run via cargo (during dev)
cargo run -p cli -- inventory

# install on $PATH
cargo install --path cli
dobj inventory
```

The release workflow ships `dobj-{target}.tar.gz` per platform — just
the binary, no shared lib bundling needed (it's a pure HTTP client with
no SCIP dependency).

## End-to-end example

```bash
# bring up the daemon stack
just dev   # in another terminal

# query state
dobj status
dobj actions
dobj inventory

# run an action and see its progress streamed
dobj run craft-basics::FindLog
# [generateProof/running] Verifying Inputs
# [generateProof/done]   Proof generation complete
# [commit/running]       Awaiting blob landing
# [commit/done]          Commit complete
# action:   craft-basics::FindLog
# run id:   3f8c…
# old root: 0xabcd...
# new root: 0xefgh...
# outputs:
#   + craft-basics_log_0x70e35...c96b.dobj

# tail the event hub while you do other things
dobj events
# {"type":"run-action-progress","runId":"CraftWood",...}
```

## Relationship to other crates

| Crate                   | Role                                                           |
| ----------------------- | -------------------------------------------------------------- |
| [`dobjd`](../dobjd)     | the daemon this CLI talks to                                   |
| [`driver`](../driver)   | the underlying Rust library `dobjd` wraps                      |
| [`mcp`](../mcp)         | MCP server (also in `dobjd`); the CLI doesn't speak MCP itself |
| [`app-gui`](../app-gui) | React frontend that hits the same dobjd HTTP API               |

Adding a new dobjd HTTP route generally means adding a corresponding
CLI subcommand here and the matching MCP tool in [`mcp/`](../mcp). The
[top-level README parity table](../README.md#api-parity-across-surfaces)
tracks where each driver capability is exposed.
