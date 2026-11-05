# Examples

Applications on the Digital Objects Network ship as **plugins**: `.pexe`
archives the driver loads from `~/.dobj/actions/` at startup. A `.pexe` is a
zip of two files:

- **`manifest.toml`**: the plugin's name and version, its **classes** (the
  object types it defines, with emoji and descriptions), its **actions**, and
  the `module_hash` that pins the exact proof logic the script compiles to.
- **`plugin.rhai`**: the action logic as a Rhai script. The SDK compiles it
  into pod2 predicates, so the same script defines both what an action does
  and what its zero-knowledge proof must show.

The example applications live in the repo under
[`examples/`](https://github.com/0xPARC/digital-objects-network/tree/main/examples):

| Example                         | What it is                                                     | Classes | Actions |
| ------------------------------- | -------------------------------------------------------------- | ------- | ------- |
| [craft-basics](./craft-basics.md) | Survival-crafting starter                                    | 6       | 9       |
| [craft-rocket](./craft-rocket.md) | Factory tech tree with a rocket win condition                | 53      | 84      |
| [Bitcraft](./bitcraft.md)       | Resource-production game, shipped as a desktop app with its own driver | 27      | 134     |

craft-basics and craft-rocket are installed into a driver with `dobj install`.
Bitcraft is a desktop application that bundles its own driver and plugin; see
its page.

## Installing a plugin

`dobj install` takes a local `.pexe` path or an http(s) URL, validates it, and
hot-reloads the running daemon's catalog, so no restart is needed:

```bash
# from a published release URL...
~/.dobj/bin/dobj install https://example.com/my-plugin.pexe

# ...or a local file
~/.dobj/bin/dobj install ./my-plugin.pexe

# confirm the new actions are in the catalog
~/.dobj/bin/dobj actions
```

### From source

The [`pexe`](https://github.com/0xPARC/digital-objects-network/tree/main/libs/pexe)
CLI builds plugin sources into archives and can install them in one step:

```bash
git clone https://github.com/0xPARC/digital-objects-network
cd digital-objects-network

just install-plugins   # build + install every plugin under examples/

# or a single plugin:
cargo run -p pexe --release -- build --install examples/craft-rocket
```

`pexe build` also rewrites the `module_hash` in each manifest to match the
hash the compiled script actually produces, so the manifest always matches
what the driver will accept.

> **Warning:** an action's proof logic is pinned by its `module_hash`, and
> class and action **names are part of that identity**. Renaming a class or
> action in a published plugin invalidates every existing object and recorded
> proof of that type, so treat published names as frozen.
