# bitcraft

A privacy-preserving crafting game where each item is a **digital object**
backed by a zero-knowledge proof. Items are `.dobj` JSON files on disk;
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

- **`dobjd`** ([dobjd/](dobjd)) — long-running driver process. Serves the
  REST/SSE API on `:7717` and the MCP server on `:7718`. Owns the
  filesystem watcher, plugin loader, RocksDB, and the in-memory state
  every client shares.
- **`dobj`** ([dobj/](dobj)) — terminal CLI. Subcommands for inventory,
  inspecting objects/classes, running actions, watching the event bus,
  and managing the daemon (`start`/`stop`/`status`/`logs`).
- **Desktop / web** ([app-gui/](app-gui)) — React UI bundled either as a
  Tauri shell or served from Vite. Both modes call dobjd over HTTP/SSE.
- **MCP** — `bitcraft-mcp-proxy` (in [mcp/](mcp)) bridges Claude Desktop's
  stdio transport to dobjd's HTTP MCP server. Claude Code connects to
  `http://127.0.0.1:7718/mcp` directly via `claude mcp add`.
- **Hosted synchronizer + relayer** — public endpoints the daemon points
  at by default. The synchronizer maintains the canonical Merkle trees
  of transactions + nullifiers; the relayer submits proof payloads as
  EIP-4844 blobs. Both are independent Rust services
  ([synchronizer/](synchronizer), [relayer/](relayer)) that can also
  run locally for development.

## Install (end user)

Paste this prompt to any MCP-aware agent (Claude Code, Cursor, etc.):

> Read https://raw.githubusercontent.com/dobjlabs/zk-craft/main/SKILL.md and set up bitcraft.

The skill installs `dobjd`, `dobj`, `bitcraft-mcp-proxy`, and the
bundled `craft-basics` plugin into `~/.dobj/`, configures the hosted
synchronizer + relayer URLs, starts the daemon, and registers MCP with
the agent. End-to-end install is a couple of minutes.

Manual install instructions and the underlying script are in
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
git clone https://github.com/dobjlabs/zk-craft
cd zk-craft

# install GUI deps
cd app-gui && pnpm install && cd ..

# copy env templates if you want to run synchronizer/relayer locally
cp synchronizer/.env.example synchronizer/.env
cp relayer/.env.example relayer/.env
# fill in RPC_URL, BEACON_URL, TO_ADDRESS, PRIVATE_KEY as needed

just dev
```

`just dev` brings up five panes via mprocs:

| Pane | Purpose |
|---|---|
| `synchronizer` | rebuilds canonical state from chain data (Postgres-backed) |
| `relayer` | submits proof payloads as EIP-4844 blobs |
| `dobjd` | the driver daemon — HTTP on `:7717`, MCP on `:7718` |
| `web` | Vite on `:1420`, hot-reload for the React app |
| `desktop` | Tauri shell pointing at the standalone Vite |

The desktop window opens automatically. Open `http://localhost:1420` in
any browser to use the website client. MCP-aware agents can connect via
`claude mcp add --transport http bitcraft http://127.0.0.1:7718/mcp`.

Run individual pieces standalone with `just sync`, `just relayer`,
`just dobjd`, `just web`, `just desktop`.

## Workspace map

| Crate | Role |
|---|---|
| [`driver/`](driver) | the canonical Rust library — opens `~/.dobj/`, runs actions, queries inventory. Single entry point for any in-process consumer |
| [`dobjd/`](dobjd) | HTTP + MCP daemon wrapping the driver. Long-running, owns broadcast hub + file watcher |
| [`dobj/`](dobj) | terminal CLI client for dobjd |
| [`mcp/`](mcp) | MCP server library + `bitcraft-mcp-proxy` stdio bridge |
| [`app-gui/`](app-gui) | React frontend + thin Tauri shell. Fetches from dobjd over HTTP/SSE |
| [`synchronizer/`](synchronizer), [`relayer/`](relayer) | chain-side services (Postgres-backed) |
| [`txlib/`](txlib) | transaction builder — event hash chain, `TxFinalized` predicate, nullifier derivation |
| [`sdk/`](sdk) | higher-level helpers used inside plugin actions |
| [`pexe/`](pexe) | plugin packager — bundles `manifest.toml` + `plugin.rhai` into `.pexe` archives |
| [`plugins/craft-basics/`](plugins/craft-basics) | the bundled crafting plugin (Log, Wood, Stone, sticks, picks…) |
| [`common/`](common), [`pod2utils/`](pod2utils), [`intro_pods/`](intro_pods), [`timelib/`](timelib) | shared utilities + intro proof-of-work / VDF pods |

Built on **pod2** (0xPARC's predicate-of-data system) using `plonky2` +
Groth16 — proofs are constant-size regardless of input count.

## Plugins

Actions and classes come from plugin archives (`.pexe` files) loaded
from `~/.dobj/actions/`. Each plugin zips together:

- `manifest.toml` — name, version, class descriptions, action names, module hash
- `plugin.rhai` — the action logic as a Rhai script

Plugin sources live under [plugins/](plugins). The [pexe](pexe) crate
provides a CLI:

```bash
just pack-plugins       # build plugins into target/pexe/*.pexe
just install-plugins    # same, then copy to ~/.dobj/actions/
```

`install-plugins` also rewrites the `module_hash` line in each plugin's
manifest to match the hash the compiled pod2 module actually produces,
so the committed manifest always matches what the driver will accept.

To add or modify a plugin, edit the files under `plugins/<name>/` and
re-run `just install-plugins`. Restart dobjd to pick up the new catalog.

## Reset

Wipe local state and start fresh:

```bash
just reset
```

Clears local RocksDB, local object files (`~/.dobj/objects/`), local
plugin installs (`~/.dobj/actions/`), and the local Postgres databases
used by the synchronizer and relayer. The next `just dev` re-installs
plugins automatically.

If you're not using the default local `postgres` role/admin database,
either adjust the `just reset` command or drop the `synchronizer` and
`relayer` databases manually.

## Further reading

- [SKILL.md](SKILL.md) — end-user install via agent prompt
- [readmes/digital-objects.md](readmes/digital-objects.md) — full `.dobj`
  file structure spec
- [readmes/driver-design.md](readmes/driver-design.md) — driver API +
  storage model
- [readmes/synchronizer-design.md](readmes/synchronizer-design.md) — chain
  indexing + Merkle trees
- [readmes/network.md](readmes/network.md) — how transactions reach the
  chain
- [readmes/pexe-design.md](readmes/pexe-design.md) — plugin format
- [deploy/ec2/README.md](deploy/ec2/README.md) — running the
  synchronizer + relayer on an EC2 box
