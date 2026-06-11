# Contributing

Building and running the Digital Objects Network from source. End users should
start with [INSTALL.md](INSTALL.md) instead.

## Prerequisites

- Rust toolchain (nightly pin in [rust-toolchain.toml](rust-toolchain.toml))
- Node.js + [`pnpm`](https://pnpm.io/)
- [`just`](https://github.com/casey/just) and [`mprocs`](https://github.com/pvolok/mprocs)
- PostgreSQL running locally, only if you also run the chain-side services
  (synchronizer / relayer / archiver) locally. With `just dev-remote` you can
  point at the hosted endpoints instead and skip Postgres entirely.

## Clone and run

```bash
git clone https://github.com/dobjlabs/digital-objects-network
cd digital-objects-network

# install GUI deps
cd interfaces/gui && pnpm install && cd ../..

# only needed if you run the chain-side services locally:
cp services/synchronizer/.env.example services/synchronizer/.env
cp services/relayer/.env.example services/relayer/.env
cp services/archiver/.env.example services/archiver/.env
# fill in RPC_URL, BEACON_URL, and the relayer signing/destination values

just dev
```

`just dev` brings up everything via mprocs, each pane gated on the previous
one's health so they don't race to cold-build the shared proving-circuit cache:

| Pane           | Purpose                                                 |
| -------------- | ------------------------------------------------------- |
| `archiver`     | follows beacon blocks, archives blobs to the filesystem |
| `synchronizer` | rebuilds state from chain data (Postgres-backed)        |
| `relayer`      | submits proof payloads as EIP-4844 blobs                |
| `dobjd`        | the driver daemon -- HTTP on `:7717`, MCP on `:7718`    |
| `web`          | Vite on `:1420`, hot-reload for the React app           |
| `desktop`      | Tauri shell pointing at the standalone Vite             |

The desktop window opens automatically. Open `http://localhost:1420` in any
browser to use the website client. MCP-aware agents can connect via
`claude mcp add --transport http dobj http://127.0.0.1:7718/mcp`.

### Without local chain-side services

```bash
just dev-remote
```

Skips the local archiver / synchronizer / relayer and points dobjd at the
hosted public endpoints. Faster spin-up; no local Postgres or beacon needed.

### Standalone pieces

Run individual components with `just sync`, `just relayer`, `just archiver`,
`just dobjd`, `just web`, `just desktop`. Before running `just sync` /
`just relayer` standalone, run `just ensure-db` once to create their databases.

## Plugins

Actions and classes come from plugin archives (`.pexe` files) loaded from
`~/.dobj/actions/`. Each plugin zips together:

- `manifest.toml` -- name, version, class descriptions, action names, module hash
- `plugin.rhai` -- the action logic as a Rhai script

Plugin sources live under [examples/](examples). The [pexe](libs/pexe) crate
provides a CLI:

```bash
just pack-plugins       # build plugins into target/pexe/*.pexe
just install-plugins    # same, then copy to ~/.dobj/actions/
```

`install-plugins` also rewrites the `module_hash` line in each manifest to match
the hash the compiled pod2 module actually produces, so the committed manifest
always matches what the driver will accept. To add or modify a plugin, edit the
files under `examples/<name>/` and re-run `just install-plugins`, then restart
dobjd to pick up the new catalog.

## Testing

```bash
just test            # cargo test --workspace --release
just test-ignored    # run #[ignore] tests with --nocapture
just test-e2e        # the slow, full real-proof end-to-end test
```

Always run tests with `--release` -- proof generation is impractically slow in
debug. Unit tests use `MockProver`; real-proof tests are `#[ignore]`-gated.

Before committing, run `cargo fmt` and `cargo clippy --tests --examples`. Keep
code and comments ASCII-only (no em-dashes or other non-ASCII characters).

## Reset

```bash
just reset
```

Wipes local state: RocksDB and object files under `~/.dobj/`, the local
`synchronizer` + `relayer` Postgres databases, the archiver's blob directory,
and the dobj MCP registration. The next `just dev` re-creates the databases
(via `just ensure-db`) and re-installs plugins automatically.

If you are not using the default local `postgres` role/admin database, either
adjust the `just reset` command or drop the `synchronizer` and `relayer`
databases manually.
