# pexe

The `.pexe` archive format plus tooling for Digital Objects Network plugins.

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
  `compile_module` for compiling a script against its manifest and resolved
  imports, `resolve_manifest_imports` for turning declared `[[imports]]` into
  loaded modules, `import_build_order` for ordering a set of plugins so
  dependencies build first, and `set_manifest_hash` / `set_manifest_import_hash`
  for rewriting the `module_hash` lines in a manifest's TOML source. Exports
  the `PEXE_EXTENSION` const (`"pexe"`).
- **`pexe` CLI** (`src/bin/pexe.rs`) — the packaging tool invoked by the
  `just pack-plugins` / `just install-plugins` recipes.

The library is a dependency of the `driver` crate (driver calls `unpack` when
loading plugins), so `pexe` itself cannot depend on `driver`. It resolves the
install directory itself, in `default_install_dir`, which mirrors the rule in
`driver::paths`: `$DOBJ_HOME/actions` when set, else `~/.dobj/actions`.

## CLI

```bash
cargo run -p pexe --release -- <subcommand>
```

### `build`

Compiles one or more plugin source directories into `.pexe` archives.

```bash
# Build into target/pexe/*.pexe (default)
cargo run -p pexe --release -- build examples/*

# Build and install into ~/.dobj/actions/
cargo run -p pexe --release -- build --install examples/*

# Install into a custom directory
cargo run -p pexe --release -- build --install \
    --install-dir /path/to/actions examples/craft-basics

# Fail if manifest module_hash doesn't match compiled hash
# (by default the source manifest gets rewritten to match)
cargo run -p pexe --release -- build --check examples/*

# Resolve [[imports]] against archives in an extra directory, searched
# before target/pexe and the install dir
cargo run -p pexe --release -- build --deps /path/to/archives examples/craft-totem
```

Each build step:

1. Reads `manifest.toml` and `plugin.rhai` from every source directory given,
   and orders them so a plugin importing another in the same invocation builds
   after it. A cycle among them is an error.
2. Resolves the manifest's `[[imports]]` against already-built archives found
   in the `--deps` directories, the output dir, then the install dir. A plugin
   can only import an archive that exists: build the dependency first.
3. Rewrites any `[[imports]]` `module_hash` that does not match the module the
   dependency actually compiles to (or errors out under `--check`).
4. Compiles the script through `sdk::Sdk::load_module_from_src_actions` to
   derive the canonical pod2 module hash from its `CustomPredicateBatch` id.
5. If the declared `module_hash` in the manifest differs from the canonical
   one, rewrites the source `manifest.toml` in place (or errors out under
   `--check`). This keeps committed source self-consistent.
6. Zips `manifest.toml` + `plugin.rhai` into `<plugin.name>.pexe`.
7. Optionally copies the archive into the install directory.

### `dump`

Inspect the contents of a `.pexe` without installing.

```bash
cargo run -p pexe --release -- dump ~/.dobj/actions/craft-basics.pexe
```

Prints the parsed manifest (via `Debug`) and the full `plugin.rhai` source.

### `inspect`

Renders what a plugin compiles to, from either a `.pexe` archive or a source
directory.

```bash
# Podlang for every predicate (add --action to filter, --middleware for the
# compiled batch rather than the SDK-synthesized source)
cargo run -p pexe --release -- inspect predicates examples/craft-basics

# Class state-space signatures, and the action/class graph
cargo run -p pexe --release -- inspect classes examples/craft-basics
cargo run -p pexe --release -- inspect graph examples/craft-basics --format mermaid

# Mint synthetic inputs and run the multi-pod solver (mock), or prove for real
cargo run -p pexe --release -- inspect plan examples/craft-basics --action FindLog
cargo run -p pexe --release -- inspect prove examples/craft-basics --action FindLog
```

Every subcommand takes `--deps` with the same meaning as `build`: where to
look for the archives this plugin's `[[imports]]` name.

Inspecting an **archive** enforces its manifest pins, since a built archive
records exactly which modules it was compiled against. If the dependency found
on disk is not that module, the command fails rather than describing a module
that is not the one in the file. Inspecting a **source directory** does not,
because `pexe build` is what stamps its pins.

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

# One entry per module whose actions this plugin sub-calls. `name` is the
# alias the script binds it to (`subaction("craft-basics::CraftWood")`),
# local to this plugin. `module_hash` is the identity, rewritten by
# `pexe build` like `[plugin] module_hash`. The two are tied together
# today only because imports are resolved by searching installed and
# built archives, which are one per plugin name.
[[imports]]
name = "craft-basics"
module_hash = "62525b9696c1402d3b37fbad775e7d3cc915aec4346f231b0fcb57d37ef451b9"
```

Parsed by `sdk::manifest::Manifest`; see that module for the canonical field
list.

A declared import is normally called by some action, via
`action.subaction("craft-basics::CraftWood")`. One that nothing calls loads
with a warning: it still emits a `use module` line, so the archive requires
that dependency to be installed, but it contributes no predicate, so the
module hash is identical with it, without it, or with its pin repointed
elsewhere.

## Using the library

```rust
use pexe::{compile_module, dep_search_dirs, pack, resolve_manifest_imports, unpack, PluginSource};
use sdk::Sdk;

// Read a plugin from disk.
let source = PluginSource::read("examples/craft-basics")?;
let manifest = source.parse_manifest()?;

// Check what hash the script actually produces. Declared [[imports]]
// resolve against the archives already built into these directories.
let sdk = Sdk::default();
let dep_dirs = dep_search_dirs(&[], std::path::Path::new(pexe::DEFAULT_OUT_DIR), None);
let imports = resolve_manifest_imports(&sdk, &manifest, &dep_dirs)?;
let module = compile_module(&sdk, &manifest, &source.script, &imports)?;
println!("module hash: {:#}", module.module().batch.id());

// Zip a manifest + script into pexe bytes.
let bytes = pack(&source.manifest_toml, &source.script)?;

// Unpack pexe bytes to get back the parsed manifest + script.
let (manifest, script) = unpack(&bytes)?;
```

The driver's `PexeCatalog` (in `driver/src/pexe_catalog.rs`) is the
canonical consumer — see that file for the scan + per-execution-reload
pattern.
