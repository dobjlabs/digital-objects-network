# `dobjd`

The Digital Objects Network driver daemon. Wraps `Arc<driver::Driver>` behind an HTTP/SSE
API, a default browser UI, and an MCP server so every client (terminal CLI,
browser, MCP-aware agents) talks to a single driver process per machine.

`dobjd` owns `~/.dobj/` exclusively — the RocksDB lock, the in-memory
catalog, and the broadcast event hub. The companion CLI
(`dobj`, in the [`cli/`](../../interfaces/cli) crate) and the React frontend in
[`gui/`](../../interfaces/gui) are thin clients over the API surface here.

## What it does

```
browser UI ─┐
CLI ────────┼── HTTP/SSE ──► dobjd ──► ~/.dobj/
MCP agents ─┘               └─ /ui/ default or custom web assets
```

Two concurrent listeners:

- **REST/SSE on `127.0.0.1:7717`** (configurable via `DOBJD_PORT`) —
  routes mirror driver capabilities and there's a single SSE event
  stream every client subscribes to.
- **MCP on `127.0.0.1:7718`** (`DOBJD_PORT + 1`) — streamable-HTTP MCP server (from the
  [`dobj-mcp`](../../interfaces/mcp) crate) sharing the same `Arc<Driver>` as the
  HTTP routes, so an MCP-driven action shows up in real time on every
  other connected client. Gated by the `mcpEnabled` setting (default
  **off**); when off, the MCP port is not bound at all. See
  [Settings](#settings) for toggle semantics.

Release builds serve the default UI at `/ui/`. Development uses Vite on
`:1420`; custom GUIs can be served by dobjd or by their own frontend server.

For a supervised daemon, `--exit-on-stdin-close` makes stdin a parent-lifetime pipe. The supervisor must keep the write end open for the daemon's entire lifetime. Closing the pipe, including through a supervisor crash, exits the daemon immediately. The watcher starts before initialization and circuit warm-up, and does not wait for blocking work or connected HTTP/SSE clients. Standalone launches without this flag ignore stdin EOF and retain their existing lifetime behaviour. `--version` remains a read-only command that exits before installing the watcher.

## HTTP API

All routes return JSON unless noted; errors come back as
`{"error": "..."}` with an appropriate status code.

| Method | Path                            | Driver call                                                  |
| ------ | ------------------------------- | ------------------------------------------------------------ |
| `GET`  | `/objects`                      | `sync_objects` (with fallback to `list_objects`)             |
| `GET`  | `/objects/unspent`              | as `/objects`, excluding nullified (spent) objects           |
| `GET`  | `/actions`                      | `list_actions`                                               |
| `GET`  | `/state-root`                   | `get_state_root`                                             |
| `GET`  | `/objects/dir`                  | `paths().objects_dir`                                        |
| `POST` | `/objects/import`               | `import_object` (body: `{ "dobj": "<json>", "fileName"?: "<name>" }`; see below) |
| `GET`  | `/objects/{file_name}`          | `read_object(&Path)` (basename in `~/.dobj/objects/`)        |
| `GET`  | `/objects/{file_name}/raw`      | `read_object_raw(&Path)` (file contents verbatim)            |
| `POST` | `/objects/{file_name}/discard_unsafe` | `discard_object_unsafe(&Path)` (see below; no state guarantees) |
| `GET`  | `/classes`                      | `list_classes`                                               |
| `GET`  | `/classes/{name}`               | `get_class`                                                  |
| `GET`  | `/settings`                     | `load_settings`                                              |
| `PUT`  | `/settings`                     | `save_settings`                                              |
| `POST` | `/runs`                  | starts a run, returns `202 { runId, status }` (non-blocking) |
| `GET`  | `/runs`                 | retained runs, oldest first (`?status=active`: in-flight)    |
| `GET`  | `/runs/{run_id}`        | run status + result/error + progress log (poll)              |
| `GET`  | `/runs/{run_id}/events` | per-run SSE: replays buffered progress then tails live       |
| `GET`  | `/actions/{id}`                 | `get_action`                                                 |
| `GET`  | `/actions/{id}/feasibility`     | `check_action`                                               |
| `POST` | `/proof-jobs`                       | starts a proof job (`prove` then `record`), returns `202` with its state; body as `/runs`'s `input` |
| `GET`  | `/proof-jobs`                       | retained proof jobs, oldest first (`?status=active`: unfinished) |
| `GET`  | `/proof-jobs/{job_id}`              | job state; `?wait=true&timeoutMs=` holds until it is done or failed |
| `POST` | `/transactions/submit`                  | `submit` each of `{ "txFinals": [...] }`, returns their records |
| `GET`  | `/transactions`                 | `list_transactions`: open and recently completed records, oldest first      |
| `GET`  | `/transactions/{tx_final}`      | `get_transaction`; `?wait=true&timeoutMs=` holds until landed or expired |
| `DELETE` | `/transactions/{tx_final}`    | `delete_transaction`: abandon a recorded transaction never offered to the relayer |
| `GET`  | `/events`                       | every state change (SSE), numbered by `seq`                  |

### Imported objects must match their content hash

`POST /objects/import` refuses (`400`) a file whose `contentHash` is not the commitment of its `obj`, the hash the store and the chain know the object by. Files dobjd writes always match, so this only refuses edited or hand-made files.

### Imported objects keep the name they are given

`POST /objects/import` files the object under `fileName` when the request has one, typically the name of the file the object came in, and otherwise under a name derived from its commitment, as action outputs are named. A given name gets `.dobj` added when it lacks it, and `name 2.dobj`, `name 3.dobj` and so on when another file, live, spent or discarded, already has it. It is refused (`400`) when it is empty, starts with `.`, contains a path separator, control character or a character Windows forbids, is a Windows device name, is longer than 200 bytes, or has the form of a derived name without being this object's. The name lasts until an action spends the object: its outputs get derived names.

### Imported objects share their key

An imported object's file carries its spending key, so whoever exported it can still spend it. `POST /objects/import` records a hash of the object's key in `imported-keys` in the data directory, and every object summary reports `sharedKey: true` while the object's key is one of those. An action that keeps the key keeps the flag, as most mutations do unless the plugin's script sets a new key. An action that replaces the key, such as the class's built-in `Rekey<Class>` action, clears it.

### Discarding objects is unsafe

`POST /objects/{file_name}/discard_unsafe` moves an unspent object's file straight into `~/.dobj/objects/.discarded/`, whatever its status (`live`, `pending`, or `unknown`) and even while a run holds it, and returns the object's summary from before the move. Spent (`nullified`) objects are left alone and come back as `404`. It is an escape hatch for clients that have decided an object is stale, such as an `unknown` output left by a crash before its transaction was recorded, not an object-state transition. An output of a recorded transaction needs no discarding: `DELETE /transactions/{tx_final}` removes it with the record, and settlement removes it when the transaction expires.

None of the guarantees dobjd otherwise keeps about object state are promised
for it:

- nothing is checked against the chain or reconciled first, so a live object,
  or a pending one whose transaction later lands, is dropped all the same;
- a run holding the object as an input is not told or stopped;
- discarded files are never listed, synced, or used as action inputs again;
  to recover one, re-import it with `POST /objects/import`.

A missing or spent object is a `404`.

### Transactions

An action goes through four stages, each its own driver call: `prove` (hold the inputs, fetch the grounding witness, prove, shrink), `record` (write the outputs as `unknown`, the payload and the record under `~/.dobj/transactions/<txFinal>/`), `submit` (post the payload to the relayer) and `settle` (query the synchronizer for the status of attempted transactions and check only head-based expiry for never-submitted records). The record is the durable state: `recorded` (proved, not yet posted), `submitted` (the relayer holds a job; `relayerJobId`, and `txHash` once broadcast), then `landed` (outputs `live`, inputs `nullified`) or `expired` (the synchronizer rejected the included proof, or its head is more than `MAX_STATE_ROOT_AGE_BLOCKS` past the grounding block; either way it can never land, so the output files are removed and the inputs are free again, and `reason` says which). A rejection is final because every check the synchronizer applies, grounding age and collisions with committed state, only gets stricter as the chain grows. A relayer job that fails is an annotation (`relayerFailure`) on a `submitted` record, not an end: a failed job may still have been broadcast, and only the chain decides.

Clients choose when to prove and first submit transactions. A client that wants two proofs in flight and submissions batched runs `POST /proof-jobs` and `POST /transactions/submit` itself; the built-in run is one composition of the same stages. Settlement runs in the daemon: a task wakes when transaction state changes. While any submission is unresolved it repeats every 1.5 seconds; when only never-submitted records are open it checks the head every 30 seconds. Submission failures wake the task so retry handling starts promptly. A transaction that acquires a submission attempt during a pass waits for the next pass before settlement can decide its outcome. `GET /objects` still reconciles every object file before listing, so explicit inventory refreshes can query membership even when no transaction has been submitted. An object listing shows an `unknown` output of a `submitted` transaction as `pending`, with the transaction's `txHash`. Object files also retain their known creation hash, so it stays available after the transaction record is pruned. A landed record takes `txHash` from the synchronizer's status, which names the Ethereum transaction that included the proof, so it has one even when the submission acknowledgement and job ID were lost.

A `recorded` transaction can be deleted until a submission is attempted; after that the relayer may hold the payload, so the record stays until the chain lands or outruns it. The daemon retries attempted submissions whose acceptance was not recorded, with exponential backoff up to 60 seconds between attempts. Retries resume after a restart and stop on acceptance, landing, or expiry. At most four retries run concurrently, outside the settlement lock, so slow submission requests do not hold up inventory refreshes or other settlement passes. Overlapping submissions for one `txFinal` wait for the in-flight call and reuse its accepted job. Calls for other transactions proceed independently. A failed request releases the submission lock while retaining the durable attempt flag, so retry remains possible and deletion stays guarded. Transactions that have never been offered to the relayer are not submitted automatically. Records survive a daemon restart; proof jobs, runs and input holds do not, and nothing on disk depends on them.

Completed (`landed` or `expired`) records and payloads are retained for ten minutes after settlement, then pruned. The daemon finishes local file reconciliation and copies known creation hashes to outputs before pruning. Successful local reconciliation is cached in memory during retention, and rechecked after a restart, whenever the record changes, and before pruning. Metadata read/write failures defer pruning. Cleanup continues when synchronizer requests fail, and never-submitted transactions retain their 30-second polling interval during an outage. Open records are never pruned. A later `GET /transactions/{txFinal}` returns 404 once its receipt has been removed.

The event stream publishes `transaction-pruned` with `txFinal` when retention ends. This removes the receipt from client snapshots while preserving a completed job's outcome. `transaction-deleted` continues to mean an unsubmitted transaction was explicitly abandoned.

### Runs are non-blocking

`POST /runs` registers the run, kicks off a background worker, and returns a `runId` immediately. The worker proves and records under the run's id, submits, then follows the transaction's record until the chain decides: `succeeded` with the result once it lands, `failed` with the reason if it expires. The objects a run was given stay marked with its id (`heldByRunId`) until then. A relayer refusal is reported in the progress log and leaves the transaction `recorded`; the daemon retries submission while the run keeps following it. If submission cannot start (for example, the synchronizer head is unavailable), the run retries twice with 1.5- and 3-second delays before failing with the recorded transaction ID and a `dobj submit` recovery command. Conflicts such as an expired grounding fail immediately. These retries belong to the active run; they do not submit unrelated staged transactions.

Runs and standalone proof jobs can prove concurrently. Callers control how much work they start according to CPU/GPU and memory capacity. The initial `queued` status means the background worker has not started yet. Input holds still prevent concurrent work from spending the same object.

The worker records the run's status, ordered progress log, and terminal
result/error in an in-memory registry. Follow a run either way:

- **Poll** `GET /runs/{run_id}` for the current state (the disconnect-recovery path — a client that lost its connection re-reads the outcome here).
- **Stream** `GET /runs/{run_id}/events`, which replays the buffered progress (honoring `Last-Event-ID` on reconnect) then tails live events until the run is terminal.

Run snapshots include monotonic `elapsedMs` and ordered `stageTimings` intervals (`stage`, `status`, `startedAtMs`, `elapsedMs`). Local work separates module loading, action/statement building, proof-of-work mining, layout solving, ZK proving, and shrinking. Commit work separates file writes, submission, and waiting for settlement. The settlement interval includes relayer processing, chain inclusion, and synchronization. Mining interrupts building; a resumed building interval gets its own entry, so durations do not overlap. A supplied mining key skips the search interval, while solving and proving still run.

The active interval's elapsed time advances on each snapshot; completed intervals and total time freeze when the run finishes. Errors and worker panics mark the active interval failed. Progress events include `stage` when available and total `elapsedMs` at the event, shared by HTTP SSE and MCP run reads. These are elapsed measurements, with no percentage or ETA.

Each accepted `POST` mints a fresh `runId`; clients don't choose it. Terminal runs and proof jobs are retained for a short TTL then reaped; both are in-memory only, and the transaction records they produced are on disk.

### Events

`GET /events` is one stream of every state change, each event numbered by `seq` (also the SSE `id`), so a client that sees a gap knows to re-read:

- `run-action-progress`: a run's progress log entry.
- `proof-job`: a proof job's state, on every change.
- `transaction`: a transaction record, on every change (recorded, submitted, tx hash known, relayer failure, landed, expired).
- `transaction-deleted`: `{ txFinal }`.
- `object`: an object's summary, whenever its status, tx hash or holder changes (a hold taken or released, a record written, a settlement pass).
- `object-removed`: `{ fileName }`, an output of an expired or deleted transaction, or a discarded object.

A client can keep its object list current from `object` and `object-removed` alone, re-listing only when its stream reconnects.

## MCP integration

The `mcp` module ([`src/mcp.rs`](src/mcp.rs)) is the glue between this crate and [`mcp/`](../../interfaces/mcp). `DobjdOps` implements `DobjOps` against the same `Arc<Driver>`, registries and event hub the HTTP routes use, so `start_run` starts a run (returning a `runId`) and `get_run` polls it — an MCP-driven action is the same run object the browser UI / `dobj` CLI can follow, and its progress fans out over the shared SSE hub in real time. The stage tools (`start_proof_job`, `get_proof_job`, `submit_transactions`, `get_transaction`, ...) mirror `/proof-jobs`, `/transactions/submit` and `/transactions`; their `wait_ms` is capped below MCP clients' tool-call limits, so a held call answers with the current state and the agent calls again.

## Hardware requirements

| Resource | Minimum | Notes                                                                 |
| -------- | ------- | --------------------------------------------------------------------- |
| CPU      | 4 cores | plonky2 proof generation is parallel and benefits from multiple cores |
| RAM      | 8 GB    | proof generation peaks at several GB; 16 GB recommended               |
| Disk     | 2 GB    | `~/.dobj/` RocksDB store and object files                             |

## Build and run

```bash
# from the workspace root
cargo run --release -p dobjd

# or via the just recipe (matches what `just dev` uses)
just dobjd

# different HTTP port; MCP binds to the adjacent port, 127.0.0.1:7728:
DOBJD_PORT=7727 cargo run --release -p dobjd
```

Released binaries are signed + notarized on macOS. The bundle is just
the four executables (`dobjd`, `dobj`, `dobj-mcp-proxy`, `pexe`).
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
  "synchronizerApiUrl": "https://terminus.digiobjects.net:4000",
  "relayerApiUrl": "https://terminus.digiobjects.net:4200",
  "mcpEnabled": false,
  "cpuUsage": "medium"
}
```

Compile-time defaults come from `DEFAULT_SYNCHRONIZER_API_URL` /
`DEFAULT_RELAYER_API_URL` env vars at build time (set by the release
workflow), overridden by anything in the settings file at runtime.

Saving `mcpEnabled` through any settings write path (`PUT /settings`,
`dobj settings set --mcp`, the MCP `write_settings` tool) starts or
stops the embedded MCP server on the spot — no daemon restart. On the
HTTP path, enabling fails (and nothing is persisted) if the adjacent
port is already taken. The MCP tool instead persists first and
reconciles in the background, so a self-disabling agent's final
response can flush before the server stops.

`cpuUsage` (`light`, `medium` or `full`, default `medium`) sets how hard proving may work the machine, and each level means the same thing on every platform: `full` proves as fast as it can and may slow other applications or spin up fans; `medium` gives way to other applications and runs cooler, at a moderate cost in speed; `light` proves only on spare capacity, near silently and several times slower. Proofs, including their single-threaded parts such as PoW grinding, run in one thread pool shaped by the level; the startup circuit warm-up always runs at `full`. How a platform delivers a level differs: a thread QoS class on macOS, a thread priority plus EcoQoS on Windows, a thread priority on Linux, and fewer threads wherever those alone do not lower power enough (see [`libs/driver/src/cpu_usage.rs`](../../libs/driver/src/cpu_usage.rs)). A change applies from the next proof; proofs already running finish at the level they started with. `just cpu-scaling` measures the levels on a given machine.

Both `PUT /settings` and the MCP `write_settings` tool take a partial
body and merge it onto the current settings: any field you omit keeps
its stored value, so a write that leaves out `mcpEnabled` will not stop
a running MCP server.

## Web UI

Release builds embed the Vite app (`interfaces/gui/dist`) in the daemon via
`--features bundled-ui`. `just build-dobjd` builds both. The browser UI is
at `http://127.0.0.1:7717/ui/`; `/` redirects there. A plain Cargo build without
that feature still serves the API, but `/ui/` reports that no default UI was
bundled. It does not silently embed a stale build. The Bitcraft desktop build
uses `--features bundled-bitcraft-ui` instead, which serves the game at
`/bitcraft/`; with only that feature, `/` redirects there and `/ui/` is a 404.

The daemon serves its bundled default UI. Alternative GUIs run on their own
local web servers, such as Python's HTTP server or Vite, and connect directly
to the HTTP/SSE API. The frontend and API may use different ports. There is
no custom asset-directory setting or automatic download of GUI code.

HTTP(S) origins on `localhost`, `127.0.0.1`, or `[::1]` are trusted on any port,
so a local frontend server can use the API and SSE directly. Cross-origin
responses allow the requesting local origin. Remote and opaque `null` origins
are rejected before handlers run. Host validation restricts requests to
loopback plus the daemon's listening port. Native requests without Origin
remain available to CLI clients. The MCP listener's policy is independent.

The Vite dev proxy is a trusted local client and removes the browser Origin
before forwarding requests, including when the UI port is forwarded remotely.
Do not expose that development server to people you do not trust.

A local GUI is **trusted executable code**, not a permission sandbox. It can
access object data, including sensitive fields exposed by the API, and invoke
actions. Treat shared GUI projects as other software with access to your
objects. Browser restrictions can still apply to cross-origin local requests.

See [the frontend README](../../interfaces/gui/README.md) for build-time and
runtime API URL configuration, including hosting the default build separately.
