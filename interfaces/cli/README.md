# `cli` (binary: `dobj`)

Terminal client for [`dobjd`](../../services/dobjd). Thin HTTP wrapper around the
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

| Command                                         | Hits                                                                                                                |
| ----------------------------------------------- | ------------------------------------------------------------------------------------------------------------------- |
| `objects`                                       | `GET /objects`                                                                                                      |
| `actions`                                       | `GET /actions`                                                                                                      |
| `classes`                                       | `GET /classes`                                                                                                      |
| `inspect-object <file_name>`                    | `GET /objects/{file_name}`                                                                                          |
| `inspect-class <plugin::class>`                 | `GET /classes/{name}`                                                                                               |
| `inspect-action <plugin::action>`               | `GET /actions/{id}`                                                                                                 |
| `feasibility <plugin::action>`                  | `GET /actions/{id}/feasibility`                                                                                     |
| `state-root`                                    | `GET /state-root`                                                                                                   |
| `objects-dir`                                   | `GET /objects/dir`                                                                                                  |
| `import <path>`                                 | `POST /objects/import` (reads the `.dobj` file locally, sends contents)                                             |
| `rename <file-name> <new-name>`                 | `POST /objects/{file_name}/rename` (`.dobj` added to the new name when missing)                                     |
| `settings get`                                  | `GET /settings`                                                                                                     |
| `settings set [--synchronizer URL] [--relayer URL] [--mcp on\|off]` | `PUT /settings` (omitted flags left unchanged; the MCP toggle starts/stops the server immediately) |
| `run <action> [paths...]`                       | `POST /runs`, then follows `/runs/{id}/events` and polls `/runs/{id}` to the terminal result |
| `prove <action> [paths...] [--wait]`             | `POST /proof-jobs`; with `--wait`, follows the job through recording |
| `proof-jobs [--active]`                          | `GET /proof-jobs`; lists retained proof jobs |
| `submit <txFinal>...`                            | `POST /transactions/submit`; submits recorded transactions in order |
| `transactions`                                  | `GET /transactions`; lists durable records |
| `inspect-transaction <txFinal> [--wait]`          | `GET /transactions/{txFinal}`; optionally waits for landing or expiry |
| `delete-transaction <txFinal>`                   | `DELETE /transactions/{txFinal}`; abandons a record before any submission attempt |
| `runs list [--active]`                         | `GET /runs`, optionally filtered to active runs |
| `runs show <run-id>`                           | `GET /runs/{id}` |
| `runs wait <run-id>`                           | Follows an existing run through SSE and a final state read; never submits an action |
| `events`                                        | `GET /events` SSE — prints every event as JSON lines                                                                |

Each command renders human-friendly output by default. Pass `--json` for machine-readable output (suitable for `jq`). Inventory includes object fields, `heldByRunId` and `sharedKey`; actions include outputs, mutation slots, and argument names.

`run --json` and `runs wait --json` print the final `RunState` to stdout, with progress on stderr. Failed runs print their state and exit nonzero. `runs show` can inspect a failed run without treating that status as a CLI failure. Run IDs are printed to stderr when submitting; `--quiet` suppresses progress and the ID announcement.

Keep the CLI process running while an action executes. Shell-capable agents can use their host's background-process support and collect the result when the process exits. If interrupted, use `runs list --active --json` and `runs wait <run-id> --json` to reattach. Disconnecting does not cancel an action; inspect runs and inventory before resubmitting. Runs are in memory and retained for about ten minutes after completion, until daemon restart. SSE carries progress; bounded HTTP requests and a reconnect/poll fallback recover the final state when the stream is interrupted.

Progress includes total elapsed seconds at each boundary. Final states and `runs show --json` include `elapsedMs` and `stageTimings`, separating mining, building, solving, proving, shrinking, and network waits. Active intervals advance in snapshots; completed durations are fixed. These measurements do not estimate time remaining.

### Daemon lifecycle — operates on local files, not HTTP

| Command            | What it does                                                                                       |
| ------------------ | -------------------------------------------------------------------------------------------------- |
| `ui [--no-open]`   | starts dobjd if needed and opens `/ui/` in the default browser (or prints the URL) |
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
2. `~/.dobj/bin/dobjd` (where the installer puts it)
3. `dobjd` next to the running `dobj` (works for `cargo install`)
4. Bare `dobjd` in `$PATH`

## Build and run

```bash
# build the binary
cargo build --release -p cli

# run via cargo (during dev)
cargo run -p cli -- objects

# install on $PATH
cargo install --path cli
dobj objects
```

The release workflow ships `dobj-{target}.tar.gz` per platform — just
the binary, no shared lib bundling needed (it's a pure HTTP client).

## End-to-end example

```bash
# bring up the daemon stack
just dev   # in another terminal

# query state
dobj status
dobj actions
dobj objects

# run an action: POST returns a run handle immediately, then the CLI
# follows the run's SSE stream and polls it to the terminal result
dobj run craft-basics::FindLog
# run id:   3f8c…
# [generateProof/running] Verifying Inputs
# [generateProof/done]   Proof generation complete
# [commit/running]       Awaiting blob landing
# [commit/done]          Commit complete
# action:   craft-basics::FindLog
# run id:   3f8c…
# grounding root: 0xabcd...
# outputs:
#   + craft-basics_log_0x70e35...c96b.dobj

# tail the event hub while you do other things
dobj events
# {"type":"run-action-progress","runId":"CraftWood",...}
```

The grounding root is the state used to prove the transaction. Run completion checks the transaction's membership in synchronized state; it does not return a post-transaction state root. Use `dobj state-root` to read the current head, which may also include other transactions.

## Relationship to other crates

| Crate                           | Role                                                           |
| ------------------------------- | -------------------------------------------------------------- |
| [`dobjd`](../../services/dobjd) | the daemon this CLI talks to                                   |
| [`driver`](../../libs/driver)   | the underlying Rust library `dobjd` wraps                      |
| [`mcp`](../mcp)                 | MCP server (also in `dobjd`); the CLI doesn't speak MCP itself |
| [`gui`](../gui)                 | React frontend that hits the same dobjd HTTP API               |

Adding a new dobjd HTTP route generally means adding a corresponding
CLI subcommand here and the matching MCP tool in [`mcp/`](../mcp). The
[top-level README parity table](../../README.md#api-parity-across-surfaces)
tracks where each driver capability is exposed.
