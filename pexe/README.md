# pexe

The `.pexe` archive format plus tooling for bitcraft plugins.

A **pexe** is a zip with exactly two entries:

| Entry           | Contents                                                              |
| --------------- | --------------------------------------------------------------------- |
| `manifest.toml` | Static metadata — plugin name, version, module hash, classes, actions |
| `plugin.rhai`   | Action logic as a Rhai script, using the `sdk` crate's host functions |

The driver scans `~/.dobj/actions/*.pexe` at startup, unpacks each archive,
compiles the script via `sdk::Sdk::load_module_from_src_manifest` (which
enforces the declared `module_hash`), and aggregates the results into its
`ActionCatalog`.

## Crate layout

This crate ships a small library and a CLI in a single package:

- **`pexe` library** — archive format helpers: `pack`, `unpack`, `unpack_raw`,
  `install`, plus `PluginSource` for reading a plugin from disk,
  `compile_module_hash` for deriving the canonical hash of a script, and
  `set_manifest_hash` for rewriting the `module_hash` line in a manifest's
  TOML source. Layout-agnostic — it does not know about `~/.dobj/` or any
  other filesystem convention. Exports the `PEXE_EXTENSION` const (`"pexe"`).
- **`pexe` CLI** (`src/bin/pexe.rs`) — the packaging tool invoked by the
  `just pack-plugins` / `just install-plugins` recipes.

The library is a dependency of the `driver` crate (driver calls `unpack` when
loading plugins), so `pexe` itself cannot depend on `driver`. The CLI
therefore carries two small cross-referenced constants (`DRIVER_DOBJ_HOME_DIR`,
`DRIVER_ACTIONS_DIR`) that mirror the canonical values in
`driver::paths`.

## CLI

```bash
cargo run -p pexe --release -- <subcommand>
```

### `build`

Compiles one or more plugin source directories into `.pexe` archives.

```bash
# Build into target/pexe/*.pexe (default)
cargo run -p pexe --release -- build plugins/*

# Build and install into ~/.dobj/actions/
cargo run -p pexe --release -- build --install plugins/*

# Install into a custom directory
cargo run -p pexe --release -- build --install \
    --install-dir /path/to/actions plugins/craft-basics

# Fail if manifest module_hash doesn't match compiled hash
# (by default the source manifest gets rewritten to match)
cargo run -p pexe --release -- build --check plugins/*
```

Each build step:

1. Reads `manifest.toml` and `plugin.rhai` from the source directory.
2. Compiles the script through `sdk::Sdk::load_module_from_src_actions` to
   derive the canonical pod2 module hash from its `CustomPredicateBatch` id.
3. If the declared `module_hash` in the manifest differs from the canonical
   one, rewrites the source `manifest.toml` in place (or errors out under
   `--check`). This keeps committed source self-consistent.
4. Zips `manifest.toml` + `plugin.rhai` into `<plugin.name>.pexe`.
5. Optionally copies the archive into the install directory.

### `dump`

Inspect the contents of a `.pexe` without installing.

```bash
cargo run -p pexe --release -- dump ~/.dobj/actions/craft-basics.pexe
```

Prints the parsed manifest (via `Debug`) and the full `plugin.rhai` source.

## Manifest format

```toml
[plugin]
name = "craft-basics"
version = "0.1.0"
# Rewritten by the `pexe build` CLI to match the compiled module's batch id.
module_hash = "62525b9696c1402d3b37fbad775e7d3cc915aec4346f231b0fcb57d37ef451b9"

[[classes]]
name = "Log"
emoji = "🌲"
description = "A discovered log that can be refined into wood."

[[actions]]
name = "FindLog"
emoji = "🌲"
description = "Discover a log object by proving a short VDF."

[[actions]]
name = "UseWoodPick"
emoji = "⛏️"
description = "Internal durability/work update for wood pick usage."
hidden = true   # excluded from the user-facing action list
```

Parsed by `sdk::manifest::Manifest`; see that module for the canonical field
list.

## Using the library

```rust
use pexe::{pack, unpack, PluginSource, compile_module_hash};

// Read a plugin from disk.
let source = PluginSource::read("plugins/craft-basics")?;
let manifest = source.parse_manifest()?;

// Check what hash the script actually produces.
let hash = compile_module_hash(&manifest, &source.script)?;
println!("module hash: {hash}");

// Zip a manifest + script into pexe bytes.
let bytes = pack(&source.manifest_toml, &source.script)?;

// Unpack pexe bytes to get back the parsed manifest + script.
let (manifest, script) = unpack(&bytes)?;
```

The driver's `PexeCatalog` (in `driver/src/pexe_catalog.rs`) is the
canonical consumer — see that file for the scan + per-execution-reload
pattern.
