# GUI

Desktop GUI for browsing local objects, running actions, and committing results.

The app is a Tauri shell around:

- React frontend (`src/`)
- Rust command layer (`src-tauri/`)
- local object store (`~/.objects`)

## Current architecture

Main frontend panels:

- `InventoryPanel`: local live/nullified `.dobj` objects, drag source
- `ContextPanel`: selected object/action details, input binding, run button
- `ActionGrid`: action catalog + search/filter
- `ProofRunnerPanel`: run status UI + CPU stats + global state root display

State:

- Zustand store in `src/shared/state/store.ts` (`useStore`)

Inventory/actions come from backend via `load_gui_inventory`.

## Backend commands used by the UI

From `src-tauri/src/lib.rs` invoke handler:

- `load_gui_inventory`
- `run_sdk_action`
- `get_global_state_root`
- `sample_app_cpu`
- `get_objects_dir`
- `open_objects_dir`
- `pick_dobj_file_path`
- `read_dobj_file`
- `get_app_settings`
- `save_app_settings`

Events:

- `run-sdk-action-progress`
- `objects-changed`
- `open-settings`

## Runtime flow

Action execution (`run_sdk_action`) does:

1. Read synchronizer state.
2. Validate input objects and grounding.
3. Execute action in `craft_sdk`.
4. Submit proof payload to relayer.
5. Wait for synchronizer commit.
6. Write outputs to `~/.objects` and move consumed inputs to `~/.objects/.nullified`.
7. Emit progress events to UI.

In addition to action runs, UI polling does:

- CPU sample every `1s` (`sample_app_cpu`)
- global state root every `4s` (`get_global_state_root`)

## Config

App config (`settings.json`) contains:

- `synchronizerApiUrl` (default `http://127.0.0.1:3000`)
- `relayerApiUrl` (default `http://127.0.0.1:3200`)

Editable via Settings dialog.

## Run

From repo root:

```bash
just gui
```

Or from `app-gui/`:

```bash
pnpm tauri dev --release
```

Full stack (sync + relayer + gui):

```bash
just dev
```

## Frontend scripts

- `pnpm dev` (Vite only)
- `pnpm tauri dev`
- `pnpm gen:ids`
- `pnpm build`

## Prereqs

- Rust toolchain
- Node + pnpm
- synchronizer + relayer reachable for commit flow
