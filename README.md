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
- In `synchronizer/.env`:
  - `RPC_URL`
  - `BEACON_URL`
  - `TO_ADDRESS`

`TO_ADDRESS` must match in both `synchronizer/.env` and `relayer/.env`.

If you do not want to create the local `postgres` role, also set:

- In `relayer/.env`:
  - `DB_URL`
- In `synchronizer/.env`:
  - `SYNC_METADATA_DB_URL`

## Groth16 Setup (optional)

The driver supports two proof backends: **Plonky2** (default) and **Groth16**.
Groth16 proofs are ~600 bytes vs ~120 KiB for Plonky2, but require a one-time
trusted setup that generates ~1.6 GB of artifacts in `~/.cache/pod2-groth16/`.

```bash
just groth16-setup
```

This takes ~10 minutes. Once complete, enable Groth16 by setting `"proofType": "groth16"` in `~/.dobj/settings.json`.

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

If you are not using the default local `postgres` role/admin database, either
adjust the `just reset` command or drop the `synchronizer` and `relayer`
databases manually.
