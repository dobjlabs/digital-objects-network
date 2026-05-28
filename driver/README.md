# `driver`

Headless Rust library for working with local digital objects.

This crate is the non-Tauri backend used by `app-gui`. It owns:

- local object storage under `~/.dobj/objects`
- settings loading/saving
- the built-in action and class catalog
- synchronizer and relayer HTTP integration
- synchronous action execution and rollback

It is intentionally a library, not a daemon and not a CLI.

## What It Does

`Driver` gives callers a blocking API for:

- opening the default local store
- listing and reading `.dobj` files
- listing actions and classes from the built-in world
- checking whether an action is feasible with the current local inventory
- syncing local inventory state against the synchronizer
- importing an external `.dobj` (one not produced by this driver) — validates
  class identity + on-chain grounding, then files it under a canonical name
  derived from its commitment
- executing an action end to end:
  - resolve inputs
  - fetch grounding witness
  - generate proof
  - build relayer payload
  - stage output and nullified files
  - submit to relayer
  - wait for relayer confirmation
  - wait for synchronizer observation
  - rollback staged files if any step fails

Because the API is blocking, GUI callers should run it on a worker thread, for example with `spawn_blocking`.

## Public Entry Points

Main type:

- `driver::Driver`

Open the driver:

- `Driver::open_default()`
- `Driver::open(paths, deps)`

Core read APIs:

- `load_settings()`
- `save_settings()`
- `list_objects(query)`
- `read_object(selector)`
- `read_object_file(path)`
- `sync_inventory(query)`
- `list_actions(query)`
- `list_classes()`
- `get_class(name)`
- `check_action(action_id)`
- `get_state_root()`
- `import_object(dobj_json)` — adopt an external `.dobj`; rejects if class
  identity doesn't match, the object is already held, or its nullifier is
  already spent on-chain. Tolerates an unreachable synchronizer by importing
  the object as `Unknown`

Execution APIs:

- `execute(input)`
- `execute_with_reporter(input, reporter)`

Important public types are defined in [`src/types.rs`](src/types.rs):

- `DriverPaths`
- `DriverSettings`
- `ObjectSelector`
- `ObjectQuery`
- `ActionQuery`
- `ObjectSummary`
- `ObjectDetail`
- `ActionSummary`
- `ClassSummary`
- `CheckActionReport`
- `ExecuteActionInput`
- `ExecuteActionResult`
- `ExecutionReporter`

## Defaults

Default paths are defined in [`src/paths.rs`](src/paths.rs).

- settings file: `~/.dobj/settings.json`
- objects dir: `~/.dobj/objects`
- nullified dir: `~/.dobj/objects/.nullified`

Current settings are intentionally small:

- `synchronizer_api_url`
- `relayer_api_url`

## Catalog

The shipped catalog is the built-in crafting world implemented in [`src/builtin.rs`](src/builtin.rs).

The catalog surface is abstracted behind `ActionCatalog` in [`src/catalog.rs`](src/catalog.rs), so another source such as `.pexe` files can be added later without changing the `Driver` API.

## Network Clients

Synchronizer and relayer integrations are split into separate modules:

- [`src/clients/synchronizer_client.rs`](src/clients/synchronizer_client.rs)
- [`src/clients/relayer_client.rs`](src/clients/relayer_client.rs)

The driver depends on traits, not the HTTP implementations directly:

- `SynchronizerClient`
- `RelayerClient`

`DriverDeps` also allows overriding the payload builder used during execution. That exists mainly to keep orchestration tests fast and isolated.

## Storage Model

Object files stay backward-compatible with the existing `.dobj` JSON schema. The parser and serializer live in [`src/object_record.rs`](src/object_record.rs), and filesystem placement logic lives in [`src/object_store.rs`](src/object_store.rs).

Live objects are stored in:

- `~/.dobj/objects`

Consumed objects are moved to:

- `~/.dobj/objects/.nullified`

File names are preserved as:

- `<class>_<object_commitment>.dobj`

## Minimal Example

```rust
use driver::{Driver, ExecuteActionInput, ObjectSelector};

fn main() -> anyhow::Result<()> {
    let driver = Driver::open_default()?;

    let inventory = driver.list_objects(None)?;
    println!("objects: {}", inventory.len());

    let report = driver.check_action("CraftWood")?;
    println!("feasible: {}", report.feasible);

    let result = driver.execute(ExecuteActionInput {
        action_id: "CraftWood".to_string(),
        input_objects: vec![ObjectSelector::FileName("Log_....dobj".to_string())],
    })?;

    println!("new root: {}", result.new_root);
    Ok(())
}
```
