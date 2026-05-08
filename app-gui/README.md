# GUI

Desktop GUI for browsing local objects, running actions, and committing results.

The app is a Tauri shell around:

- React frontend (`src/`)
- Rust command layer (`src-tauri/`)
- local object store (`~/.dobj/objects`)

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
- `run_action`
- `get_global_state_root`
- `sample_app_cpu`
- `get_objects_dir`
- `open_objects_dir`
- `pick_dobj_file_path`
- `read_dobj_file`
- `get_app_settings`
- `save_app_settings`

Events:

- `run-action-progress`
- `open-settings`

## Runtime flow

Inventory loading (`load_gui_inventory`) does:

1. Read local `.dobj` files.
2. Query synchronizer membership for source txs and live nullifiers in a single request anchored to one head.
3. Promote grounded objects to `live` and auto-nullify locally-live files already spent on-chain.

Action execution (`run_action`) does:

1. Resolve and validate local input objects.
2. Request a proof-bearing grounding witness from the synchronizer for the inputs' source txs.
3. Execute the action in `craft_sdk` using that grounding witness.
4. Submit the proof payload to the relayer.
5. Wait for relayer confirmation, then wait for the synchronizer to index the new tx.
6. Write outputs to `~/.dobj/objects` and move consumed inputs to `~/.dobj/objects/.nullified`.
7. Emit progress events to the UI.

In addition to action runs, UI polling does:

- CPU sample every `1s` (`sample_app_cpu`)
- global state root every `4s` (`get_global_state_root`)

## Config

App config (`settings.json`) contains:

- `synchronizerApiUrl` (default `http://127.0.0.1:3000`)
- `relayerApiUrl` (default `http://127.0.0.1:3200`)

Editable via Settings dialog.

You can also bake different first-run defaults into the packaged app at build time:

```bash
DEFAULT_SYNCHRONIZER_API_URL=http://YOUR_HOST:3000 \
DEFAULT_RELAYER_API_URL=http://YOUR_HOST:3200 \
pnpm tauri build
```

These compile-time defaults are only used when the app has no existing `settings.json` yet.

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
- `pnpm build`

## Prereqs

- Rust toolchain
- Node + pnpm
- synchronizer + relayer reachable for commit flow
