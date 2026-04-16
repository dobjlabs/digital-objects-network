# zk-craft

Proof-driven digital objects with:

- a Tauri desktop app
- a relayer that submits proof payloads as EIP-4844 blob transactions
- a synchronizer that rebuilds app state from chain data
- a plugin system (`.pexe` archives) where each plugin ships a Rhai script
  + TOML manifest that the driver loads at startup

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

## Run

Start the full local stack:

```bash
just dev
```

This runs:

- `just sync`
- `just relayer`
- `just gui`

It also runs `just ensure-plugins` first, which installs the built-in plugins
from `plugins/` into `~/.dobj/actions/` if no `.pexe` files are present there.
On a fresh clone (or after `just reset`) this is how the GUI gets its
initial catalog of actions.

You can also run each service individually with those commands.

## Plugins

Actions and classes are defined by plugin archives (`.pexe` files) loaded from
`~/.dobj/actions/`. Each plugin is a zip of:

- `manifest.toml` — plugin metadata (name, version, class descriptions, action
  names, module hash)
- `plugin.rhai` — the action logic as a Rhai script

Plugin sources live in-repo under `plugins/<name>/`. The `pexe` crate provides
a CLI for packaging them:

```bash
just pack-plugins       # build plugins into target/pexe/*.pexe
just install-plugins    # same, then copy to ~/.dobj/actions/
```

`install-plugins` also rewrites the `module_hash` line in each plugin's
`manifest.toml` to match the hash the compiled pod2 module actually produces,
so the committed manifest always matches what the driver will accept.

To add or modify a plugin, edit the files under `plugins/<name>/` and
re-run `just install-plugins`. Restart the GUI to pick up the new catalog.

## Restarting Fresh

If you want to wipe local state and start over, run:

```bash
just reset
```

This clears local RocksDB state, local object files, local plugin
installations (`~/.dobj/actions/`), and the local Postgres databases used
by the synchronizer and relayer. The next `just dev` will re-install
plugins automatically.

If you are not using the default local `postgres` role/admin database, either
adjust the `just reset` command or drop the `synchronizer` and `relayer`
databases manually.
