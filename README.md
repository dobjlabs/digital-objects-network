# Digital Objects

The reference implementation of the **Digital Objects** network: privately-held,
stateful objects each backed by a recursive zero-knowledge proof. Items are `.dobj` JSON files on disk;
their validity is anchored to Ethereum via EIP-4844 blobs. The chain
sees only opaque commitments — an observer can't tell what an object is
or who holds it.

## Architecture

A single **driver daemon** (`dobjd`) on the user's machine owns
`~/.dobj/`. Every client talks to it the same way:

```
        ┌─ desktop app (Tauri webview shell) ─┐
        ├─ website (browser tab) ─────────────┤   HTTP/SSE
        ├─ `dobj` terminal CLI ───────────────┼──────────►  dobjd  ──►  ~/.dobj/
        └─ MCP agents (Claude, Cursor, …) ────┘            (one process)
                                                              │
                                              hosted ─────────┤
                                              synchronizer ◄──┘
                                              + relayer
```

- **`dobjd`** ([services/dobjd/](services/dobjd)) — long-running driver process. Serves the
  REST/SSE API on `:7717` and the MCP server on `:7718`. Owns the
  plugin loader, RocksDB, and the in-memory state
  every client shares.
- **`dobj`** ([interfaces/cli/](interfaces/cli)) — terminal CLI. Subcommands for objects,
  inspecting objects/classes, running actions, watching the event bus,
  and managing the daemon (`start`/`stop`/`status`/`logs`).
- **Desktop / web** ([interfaces/gui/](interfaces/gui)) — React UI bundled either as a
  Tauri shell or served from Vite. Both modes call dobjd over HTTP/SSE.
- **MCP** — `dobj-mcp-proxy` (in [interfaces/mcp/](interfaces/mcp)) bridges Claude Desktop's
  stdio transport to dobjd's HTTP MCP server. Claude Code connects to
  `http://127.0.0.1:7718/mcp` directly via `claude mcp add`.
- **Hosted synchronizer + relayer** — public endpoints the daemon points
  at by default. The synchronizer maintains the Merkle trees
  of transactions + nullifiers; the relayer submits proof payloads as
  EIP-4844 blobs. Both are independent Rust services
  ([services/synchronizer/](services/synchronizer), [services/relayer/](services/relayer)) that can also
  run locally for development.

## Install (end user)

Paste this prompt to any MCP-aware agent (Claude Code, Cursor, etc.):

> Read https://raw.githubusercontent.com/dobjlabs/digital-objects-network/main/SKILL.md and set up the Digital Objects driver.

The skill installs `dobjd`, `dobj`, `dobj-mcp-proxy`, and the
`craft-basics` plugin into `~/.dobj/` (the hosted synchronizer + relayer
URLs are baked into the binaries), starts the daemon, and registers MCP
with the agent. End-to-end install is a couple of minutes.

Prefer to install by hand? The driver and its installer scripts live in
the public releases repo,
[dobjlabs/zk-craft-releases](https://github.com/dobjlabs/zk-craft-releases#install):
a `curl ... | sh` one-liner (macOS / Linux), an `irm ... | iex` line
(Windows), and step-by-step manual instructions. The agent skill above is
[SKILL.md](SKILL.md).

## Develop (from source)

Prerequisites:

- Rust toolchain (nightly pin in [rust-toolchain.toml](rust-toolchain.toml))
- Node.js + `pnpm`
- PostgreSQL running locally (only needed if you also run the
  synchronizer/relayer locally — the default config points at hosted
  endpoints)
- [`just`](https://github.com/casey/just) and
  [`mprocs`](https://github.com/pvolok/mprocs)

```bash
git clone https://github.com/dobjlabs/digital-objects-network
cd digital-objects-network

# install GUI deps
cd interfaces/gui && pnpm install && cd ../..

# copy env templates if you want to run synchronizer/relayer locally
cp synchronizer/.env.example synchronizer/.env
cp relayer/.env.example relayer/.env
# fill in RPC_URL, BEACON_URL, TO_ADDRESS, PRIVATE_KEY as needed

just dev
```

`just dev` brings up five panes via mprocs:

| Pane           | Purpose                                             |
| -------------- | --------------------------------------------------- |
| `synchronizer` | rebuilds state from chain data (Postgres-backed)    |
| `relayer`      | submits proof payloads as EIP-4844 blobs            |
| `dobjd`        | the driver daemon — HTTP on `:7717`, MCP on `:7718` |
| `web`          | Vite on `:1420`, hot-reload for the React app       |
| `desktop`      | Tauri shell pointing at the standalone Vite         |

The desktop window opens automatically. Open `http://localhost:1420` in
any browser to use the website client. MCP-aware agents can connect via
`claude mcp add --transport http dobj http://127.0.0.1:7718/mcp`.

Run individual pieces standalone with `just sync`, `just relayer`,
`just dobjd`, `just web`, `just desktop`.

## Workspace map

| Crate                                                                       | Role                                                                                                                    |
| --------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------- |
| [`libs/driver/`](libs/driver)                                                         | the core Rust library — opens `~/.dobj/`, runs actions, queries objects. Single entry point for any in-process consumer |
| [`services/dobjd/`](services/dobjd)                                                           | HTTP + MCP daemon wrapping the driver. Long-running, owns the broadcast hub                                             |
| [`interfaces/cli/`](interfaces/cli)                                                               | terminal CLI client for dobjd (binary: `dobj`)                                                                          |
| [`interfaces/mcp/`](interfaces/mcp)                                                               | MCP server library + `dobj-mcp-proxy` stdio bridge                                                                      |
| [`interfaces/gui/`](interfaces/gui)                                                       | React frontend + thin Tauri shell. Fetches from dobjd over HTTP/SSE                                                     |
| [`services/synchronizer/`](services/synchronizer), [`services/relayer/`](services/relayer)                      | chain-side services (Postgres-backed)                                                                                   |
| [`libs/txlib/`](libs/txlib)                                                           | transaction builder — event hash chain, `TxFinalized` predicate, nullifier derivation                                   |
| [`libs/sdk/`](libs/sdk)                                                               | higher-level helpers used inside plugin actions                                                                         |
| [`libs/pexe/`](libs/pexe)                                                             | plugin packager — bundles `manifest.toml` + `plugin.rhai` into `.pexe` archives                                         |
| [`examples/craft-basics/`](examples/craft-basics)                           | the bundled crafting plugin (Log, Wood, Stone, sticks, picks…)                                                          |
| [`libs/payload/`](libs/payload), [`libs/pod2utils/`](libs/pod2utils), [`libs/intro-pods/`](libs/intro-pods) | shared utilities + intro proof-of-work / VDF pods                                                                       |

Built on **pod2** (0xPARC's predicate-of-data system) using `plonky2` —
proofs are constant-size regardless of input count.

## Plugins

Actions and classes come from plugin archives (`.pexe` files) loaded
from `~/.dobj/actions/`. Each plugin zips together:

- `manifest.toml` — name, version, class descriptions, action names, module hash
- `plugin.rhai` — the action logic as a Rhai script

Plugin sources live under [examples/](examples). The [pexe](libs/pexe) crate
provides a CLI:

```bash
just pack-plugins       # build plugins into target/pexe/*.pexe
just install-plugins    # same, then copy to ~/.dobj/actions/
```

`install-plugins` also rewrites the `module_hash` line in each plugin's
manifest to match the hash the compiled pod2 module actually produces,
so the committed manifest always matches what the driver will accept.

To add or modify a plugin, edit the files under `examples/<name>/` and
re-run `just install-plugins`. Restart dobjd to pick up the new catalog.

## Reset

Wipe local state and start fresh:

```bash
just reset
```

Clears local RocksDB, local object files (`~/.dobj/objects/`), local
plugin installs (`~/.dobj/actions/`), and the local Postgres databases
used by the synchronizer and relayer. The next `just dev` re-creates the
databases (via `just ensure-db`) and re-installs plugins automatically.

If you're not using the default local `postgres` role/admin database,
either adjust the `just reset` command or drop the `synchronizer` and
`relayer` databases manually.
