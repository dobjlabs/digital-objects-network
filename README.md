# zk-craft

Proof-driven digital objects with:

- a Tauri desktop app
- a relayer that submits proof payloads as EIP-4844 blob transactions
- a synchronizer that rebuilds app state from chain data

## Prerequisites

- Rust toolchain
- Node.js + `pnpm`
- PostgreSQL running locally
- `just`
- `mprocs`

The relayer and synchronizer will create their own Postgres databases/tables if
Postgres is running, using the URLs in their `.env` files.

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

Then fill in the required values:

- In `relayer/.env`:
  - `RPC_URL`
  - `TO_ADDRESS`
  - `PRIVATE_KEY`
- In `synchronizer/.env`:
  - `RPC_URL`
  - `BEACON_URL`
  - `TO_ADDRESS`

`TO_ADDRESS` must match in both `synchronizer/.env` and `relayer/.env`.

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

## Restarting Fresh

If you want to wipe local state and start over, run:

```bash
just reset
```

This clears local RocksDB state, local object files, and the local Postgres
databases used by the synchronizer and relayer.
