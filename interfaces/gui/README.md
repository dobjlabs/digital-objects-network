# GUI

React frontend for browsing local objects, running actions, and watching
proof generation.

The same app runs in two modes:

- **Desktop window** — Tauri shell wrapping the React app
- **Browser tab** — Vite serves the same `src/` to any browser at `:1420`

Both modes talk to **`dobjd`** over HTTP/SSE on `:7717`. The Tauri shell
holds no driver state of its own — it's a webview plus a few native
desktop conveniences. Start `dobjd` (or run `just dev`) before opening
either surface.

## Architecture

```
desktop window ─┐
                ├──►  http://127.0.0.1:7717  ──►  dobjd  ──►  ~/.dobj/
browser tab ────┘
```

```
interfaces/gui/
├── src/                    # React frontend (used by both modes)
│   └── shared/api/         # HTTP + SSE client for dobjd
└── src-tauri/              # Tauri shell — desktop-only conveniences
```

Frontend panels (`src/features/`):

- `ObjectsPanel` — local live/nullified objects, drag source, `+ Import .dobj` button
- `ActionGrid` — action catalog + search/filter
- `ContextPanel` — selected object/action details, input binding, run button
- `ProofRunnerPanel` — proof-run status, CPU stats, state root
- `SettingsModal` — synchronizer/relayer URL editor

State: Zustand store at `src/shared/state/store.ts` (`useStore`).

## What goes over HTTP vs Tauri IPC

Everything that touches `~/.dobj/` lives in `dobjd`. The Tauri shell only
provides things the browser fundamentally can't do.

**dobjd HTTP / SSE** (always — from desktop _and_ browser):

| Surface                              | Route                                      |
| ------------------------------------ | ------------------------------------------ |
| `loadObjects`                        | `GET /objects`                             |
| `loadActions`                        | `GET /actions`                             |
| `getStateRoot`                       | `GET /state-root`                          |
| `getObjectsDir`                      | `GET /objects/dir`                         |
| `getAppSettings` / `saveAppSettings` | `GET` / `PUT /settings`                    |
| `importObject`                       | `POST /objects/import`                     |
| `runAction`                          | `POST /actions/run` (returns a run handle) |
| `getRun`                             | `GET /actions/runs/{id}`                   |
| `listenRunActionProgress`            | `GET /events` firehose (SSE)               |
| `listenRunActionProgressForRun`      | `GET /actions/runs/{id}/events` (SSE)      |

`hydrateData` calls `loadObjects` + `loadActions` in parallel via
`Promise.all`.

`runAction` returns immediately with a `runId` (the proof + commit run in the
background on dobjd). `runProof` follows that run's replayable SSE stream
(`listenRunActionProgressForRun`) for live progress and polls `getRun` until
the run is terminal for the authoritative result. The shared `/events` stream
remains a firehose used for refresh triggers.

**Tauri commands** (desktop-only, declared in `src-tauri/src/lib.rs`):

| Command               | Purpose                                         |
| --------------------- | ----------------------------------------------- |
| `sample_app_cpu`      | usage % for the status bar widget               |
| `pick_dobj_file_path` | native file picker                              |
| `read_dobj_file`      | parse a picked `.dobj` (returns `ObjectRecord`) |
| `open_objects_dir`    | reveal `~/.dobj/objects/` in Finder/Explorer    |

In browser mode these reject; the relevant UI either falls back (e.g.
`openObjectsDir` returns the path as text) or the feature is unavailable
(file picker — the in-app drag-and-drop covers most cases).

## Events

| Channel               | Source                | Payload                                |
| --------------------- | --------------------- | -------------------------------------- |
| `run-action-progress` | dobjd SSE (`/events`) | `{ runId, phase, status, message, … }` |
| `open-settings`       | Tauri menu (`Cmd+,`)  | empty — opens the Settings modal       |

The SSE stream is shared across every connected client (desktop, browser,
CLI, MCP), so an action triggered by an MCP agent still updates the
desktop's progress UI in real time.

## Polling

- **CPU sample**: 1s, via `sample_app_cpu` (desktop only — browser shows zeros)
- **state root**: 4s, via `GET /state-root`

## Run

From the repo root:

```bash
just dev           # synchronizer + relayer + dobjd + Vite + Tauri shell
just desktop       # just the Tauri shell + its own Vite
just web           # just Vite (browser tab on :1420)
just dobjd         # just the daemon
```

Standalone equivalents from `interfaces/gui/`:

```bash
pnpm tauri dev --release    # desktop shell, spawns its own Vite
pnpm dev                    # Vite only
pnpm build                  # production bundle
```

`just dev` opens the desktop window automatically; for browser mode visit
`http://localhost:1420` once Vite is up.

## Config

Driver settings (`synchronizerApiUrl`, `relayerApiUrl`) live in
`~/.dobj/settings.json` and are **owned by dobjd**, not the GUI. Edit
them via the in-app Settings dialog (`Cmd+,`), which writes through to
`PUT /settings`. The CLI (`dobj settings get/set`) reads/writes the same
file.

Compile-time defaults are baked into `dobjd` via the
`DEFAULT_SYNCHRONIZER_API_URL` / `DEFAULT_RELAYER_API_URL` env vars at
_driver_ build time (see `driver/src/settings.rs`). They only apply when
no `settings.json` exists yet.

To point the frontend at a non-default `dobjd` instance, set
`VITE_DOBJD_URL` when starting Vite or building the frontend:

```bash
VITE_DOBJD_URL=http://127.0.0.1:7727 pnpm tauri dev
```

(Default: `http://127.0.0.1:7717`.)

During development, all API requests and SSE subscriptions use `/api` on
the UI's origin. Vite proxies them to `VITE_DOBJD_URL` on the machine running
Vite, so forwarding only port 1420 also works when the browser is on another
machine. Production builds connect to the configured daemon URL directly.
Restart Vite after changing `VITE_DOBJD_URL`.

If initial loading fails, the UI displays the connection error and a Retry
button rather than showing an empty object list.

## Prereqs

- Rust toolchain (workspace pin in `rust-toolchain.toml`)
- Node + pnpm
- `dobjd` running, with the synchronizer + relayer it's pointed at also reachable
