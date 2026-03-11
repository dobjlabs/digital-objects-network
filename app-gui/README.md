# zk-craft app-gui

Desktop GUI for authoring and committing zk-craft object transitions.

This app is a Tauri shell around:
- a React frontend (`src/`)
- a Rust backend command layer (`src-tauri/`)
- local object files in `~/.objects`

It coordinates local proof generation with remote commit/index services.

## What the app does

At a high level, the GUI lets a user:
1. Browse local digital objects (`.dobj`) in inventory.
2. Pick an action/recipe from the action catalog.
3. Bind input objects to action arguments (drag/drop or file picker).
4. Run the action to generate a zk proof payload.
5. Submit that payload to the relayer (EIP-4844 blob tx).
6. Wait for synchronizer confirmation.
7. Update local object files:
   - consumed inputs become `nullified`
   - new outputs are written as `live`

## Runtime flow

`run_sdk_action` (Tauri command) drives the full pipeline:
1. Load current state from synchronizer (`/v1/state/full`).
2. Validate selected inputs against action signature.
3. Ensure input source tx hashes are grounded in synchronizer state.
4. Execute action through `craft_sdk` and produce output objects.
5. Build payload bytes from the finalized tx proof.
6. Submit to relayer (`POST /api/v1/proofs`) and poll job status.
7. Poll synchronizer until `tx_final` is observed.
8. Persist updated object set back to `~/.objects` (and `~/.objects/.nullified`).

Progress is emitted to the frontend over the `run-sdk-action-progress` event.

## UI structure

- `InventoryPanel`: live/nullified local objects, drag source for inputs.
- `RecipeGrid`: available actions from `spec.rs`.
- `ContextPanel`: action detail + argument binding + run trigger.
- `ProofRunnerPanel`: run phases and summary (generate proof, commit, results).

State is managed in a Zustand store (`src/shared/state/uiStore.ts`).

## Configuration

The app stores API endpoints in app config `settings.json`:
- `synchronizerApiUrl` (default: `http://127.0.0.1:3000`)
- `relayerApiUrl` (default: `http://127.0.0.1:3200`)

These are editable from the GUI Settings dialog.

## Running locally

From repo root:

```bash
just gui
```

Or directly from this folder:

```bash
pnpm tauri dev --release
```

Typical full-stack dev mode (from repo root):

```bash
just dev
```

That starts synchronizer + relayer + gui together via `mprocs`.

## Scripts

- `pnpm dev`: frontend-only Vite dev server
- `pnpm tauri dev`: run Tauri desktop app
- `pnpm gen:ids`: regenerate TypeScript unions for action/class ids
- `pnpm build`: generate ids + typecheck + Vite build

## Prereqs

- Rust toolchain (workspace uses nightly)
- Node + pnpm
- Running `synchronizer` and `relayer` services for commit flow
