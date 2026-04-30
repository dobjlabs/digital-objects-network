# zk-craft

Proof-driven digital objects with:

- a Tauri desktop app
- a relayer that submits proof payloads as EIP-4844 blob transactions
- a synchronizer that rebuilds app state from chain data
- a risc0 zkVM guest (`craft-guest`) that proves each action; its action
  validators live in [`craft-actions`](craft-actions/) and are committed to
  the guest's `image_id` at compile time

## Prerequisites

- Rust toolchain
- Node.js + `pnpm`
- PostgreSQL running locally
- `just`
- `mprocs`

The relayer and synchronizer will create their own Postgres databases/tables if
Postgres is running, using the URLs in their `.env` files.

On macOS, [Postgres.app](https://postgresapp.com/) is an easy way to get a
local Postgres install.

## Install

Install the GUI dependencies:

```bash
cd app-gui
pnpm install
cd ..
```

Copy the example env files:

```bash
cp synchronizer/.env.example synchronizer/.env
cp relayer/.env.example relayer/.env
```

If you want the repo's default Postgres URLs to work unchanged, make your local
Postgres match them once:

```bash
createuser -s postgres
```

The services will then connect through the default local admin database
`postgres://postgres@localhost:5432/postgres` and create the `synchronizer` and
`relayer` databases automatically on first run.

Then fill in the required values:

- In `relayer/.env`:
  - `RPC_URL`
  - `TO_ADDRESS`
  - `PRIVATE_KEY`
  - `GUEST_IMAGE_ID` — get the current value with `just print-image-id`
- In `synchronizer/.env`:
  - `RPC_URL`
  - `BEACON_URL`
  - `TO_ADDRESS`
  - `GUEST_IMAGE_ID` — must match the relayer's value

`TO_ADDRESS` and `GUEST_IMAGE_ID` must match in both `synchronizer/.env` and
`relayer/.env`.

If you do not want to create the local `postgres` role, also set:

- In `relayer/.env`:
  - `DB_URL`
- In `synchronizer/.env`:
  - `SYNC_METADATA_DB_URL`

## Run

Start the full local stack:

```bash
just dev
```

This runs:

- `just sync`
- `just relayer`
- `just gui`

You can also run each service individually with those commands.

## Actions and classes

The action catalog (5 actions, 4 classes for `craft-basics`) is baked into the
[`craft-actions`](craft-actions/) crate at compile time. The risc0 guest in
[`craft-guest`](craft-guest/) is a thin entry point around
`craft_actions::guest_main`, so the guest's `image_id` commits to the action
dispatch table directly — no on-disk plugin format, no module-hash bookkeeping.

To add or modify an action:

1. Edit `craft-actions/src/actions.rs` (validator) and
   `craft-actions/src/lib.rs` (action ID, dispatch arm).
2. Rebuild the workspace. The `craft-methods` build script regenerates the
   guest binary; the new `image_id` is exposed as `craft_methods::CRAFT_GUEST_ID`.
3. Update `synchronizer/.env` (`GUEST_IMAGE_ID`) and `relayer/.env`
   (`GUEST_IMAGE_ID`) to the new hash so they accept proofs from the new guest.

The driver's view of the catalog is `driver::all_actions()` /
`driver::all_classes()` in [`driver/src/catalog.rs`](driver/src/catalog.rs).

## Restarting fresh

If you want to wipe local state and start over, run:

```bash
just reset
```

This clears local object files (`~/.dobj/objects/`) and the local Postgres
databases used by the synchronizer and relayer.

If you are not using the default local `postgres` role/admin database, either
adjust the `just reset` command or drop the `synchronizer` and `relayer`
databases manually.
