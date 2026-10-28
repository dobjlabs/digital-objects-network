# `driver`

Headless Rust library for working with local digital objects.

This crate is the backend used by `dobjd`. It owns:

- local object storage under `~/.dobj/objects`
- settings loading/saving
- the built-in action and class catalog
- synchronizer and relayer HTTP integration
- the stages of a transaction: proving, recording, submitting and settling

It is intentionally a library, not a daemon and not a CLI.

## What It Does

`Driver` gives callers a blocking API for:

- opening the default local store
- listing and reading `.dobj` files
- listing actions and classes from the built-in world
- checking whether an action is feasible with the current local objects
- syncing local objects state against the synchronizer
- importing an external `.dobj` (one not produced by this driver) — validates
  class identity + on-chain grounding, then files it under a canonical name
  derived from its commitment
- the stages of a transaction, each one call so a caller can schedule them separately:
  - `prove`: takes holds on the inputs, refusing any that is held by another proof or spent by a transaction awaiting the chain, fetches the grounding witness, runs the action and shrinks the proof. Writes nothing; the holds live in the returned value.
  - `record`: writes the outputs as `unknown`, the payload, then the record, whose file is the commit point. Refuses an input another record spends and an output file that already exists. Releases the holds either way.
  - `submit`: posts a recorded payload to the relayer, refusing a grounding the synchronizer has aged out. Overlapping calls for the same `txFinal` wait on a shared per-transaction lock and reread the record after acquiring it. Accepted submissions return the existing job; failed calls release the lock for retry. Different transactions can submit concurrently.
  - `delete_transaction`: removes a recorded transaction that was never offered to the relayer, with its output files.
  - `settle`: query membership for transactions with a submission attempt, using that response's head for expiry. Never-submitted records need only a head check; mixed passes reuse the membership head. Mark records landed or expired and update object files. A submission that starts during a head-only query waits for a membership query before any terminal decision.
  - `execute`: `prove`, `record` and `submit` in one call. Landing is reported by settlement.

Because the API is blocking, GUI callers should run it on a worker thread, for example with `spawn_blocking`.

A run spends most of its time waiting on the network, so the driver owns no scheduling: how many proofs run at once, and when payloads are submitted, is the caller's policy. What the driver keeps is what the chain's commitment model needs: the output dicts of every transaction that may still land, and the guarantee that an input is never offered to two transactions.

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
- `read_object(path)`
- `read_object_raw(path)`
- `sync_objects(query)`
- `list_actions(query)`
- `list_classes()`
- `get_class(name)`
- `check_action(action_id)`
- `get_state_root()`
- `import_object(dobj_json)` — adopt an external `.dobj`; rejects if class
  identity doesn't match, the object is already held, or its nullifier is
  already spent on-chain. Tolerates an unreachable synchronizer by importing
  the object as `Unknown`

Transaction APIs:

- `prove(input, owner, reporter)`
- `record(proved)`
- `submit(tx_final)`
- `delete_transaction(tx_final)`
- `settle()`, or `fetch_relayer_jobs()` then `settle_with_relayer_jobs(relayer_jobs)` to poll the relayer outside the caller's own locks
- `list_transactions()`, `get_transaction(tx_final)`
- `prune_transactions(current_time_ms)`, `backfill_transaction_hashes()`
- `execute(input)`

Important public types are defined in [`src/types.rs`](src/types.rs):

- `DriverPaths`
- `ObjectQuery`
- `ActionQuery`
- `ExecuteActionInput`
- `ExecutionReporter`, `ExecutionStepContext`, `NoopExecutionReporter`

The transaction stage types (`ProvedTransaction`, `DeletedTransaction`, `SettleOutcome`, `SyncedObjects`, `PrunedTransactions`, `RelayerJobs`) are in [`src/pipeline.rs`](src/pipeline.rs). Settings, summaries and transaction records (`DriverSettings`, `ObjectSummary`, `ActionSummary`, `ClassSummary`, `CheckActionReport`, `TransactionRecord`) come from the `wire-types` crate.

## Defaults

Default paths are defined in [`src/paths.rs`](src/paths.rs).

- settings file: `~/.dobj/settings.json`
- objects dir: `~/.dobj/objects`
- nullified dir: `~/.dobj/objects/.nullified`
- discarded dir: `~/.dobj/objects/.discarded`

Current settings are intentionally small:

- `synchronizer_api_url`
- `relayer_api_url`

The record contract is specified in [`model/README.md`](model/README.md).

## Catalog

The shipped catalog is `PexeCatalog` ([`src/pexe_catalog.rs`](src/pexe_catalog.rs)), which loads `.pexe` plugin archives from `~/.dobj/actions/`.

The catalog surface is abstracted behind `ActionCatalog` in [`src/catalog.rs`](src/catalog.rs), so an alternative source can be substituted without changing the `Driver` API.

## Network Clients

Synchronizer and relayer integrations are split into separate modules:

- [`src/clients/synchronizer_client.rs`](src/clients/synchronizer_client.rs)
- [`src/clients/relayer_client.rs`](src/clients/relayer_client.rs)

The driver depends on traits, not the HTTP implementations directly:

- `SynchronizerClient`
- `RelayerClient`

`DriverDeps` also allows overriding the payload builder used during execution. That exists mainly to keep orchestration tests fast and isolated.

## Storage Model

An object file says only what the chain has said about the object: `unknown`, `live` or `nullified`. The parser and serializer live in [`src/object_record.rs`](src/object_record.rs), and filesystem placement logic lives in [`src/object_store.rs`](src/object_store.rs). Whether an output is awaiting a transaction (`pending` in listings) is derived from its open transaction record. An optional `txHash` is stored in the object file and survives transaction-record pruning. Listings prefer a transaction record's newer hash while it is retained, and fall back to the file's hash. Spending an input preserves its original creation hash.

Each proved transaction has a directory under `~/.dobj/transactions/<txFinal>/` holding `payload.bin`, the protocol artifact that goes to the relayer, and `record.json`, the record served by dobjd. The record is written last, by rename, so a crash mid-write leaves at most inert output files and a directory without a record, which the driver ignores. Input holds are in memory only; a restart loses every proof in progress and nothing on disk depends on them.

`prune_transactions` repairs settled object files from durable terminal records before removing records and payloads ten minutes after settlement. Successful reconciliation is cached for unchanged terminal records during retention; a restart clears the cache, record changes invalidate it, and pruning always revalidates the files. Open transactions remain durable. Unreadable files or failed metadata writes defer pruning for retry; cleanup does not discard the evidence needed to finish local reconciliation. `backfill_transaction_hashes` uses the relayer's batch lookup (at most 256 commitments per request) to recover missing hashes for retained landed records, including those without a relayer job ID. It copies recovered hashes to output files without changing the transaction outcome or retention deadline. The daemon runs this work separately every 30 seconds, and schedules the next cleanup deadline even when no transactions are open. Pruning keeps object files and their hashes and removes only the completed transaction directory.

Live objects are stored in:

- `~/.dobj/objects`

Consumed objects are moved to:

- `~/.dobj/objects/.nullified`

`Driver::discard_object_unsafe` moves an unspent object, whatever its status, to:

- `~/.dobj/objects/.discarded`

The driver never reads that directory back, and the move keeps none of its
usual guarantees about object state: nothing is checked against the chain
or reconciled first.

File names are preserved as:

- `<class>_<object_commitment>.dobj`

## Minimal Example

```rust
use driver::{Driver, ExecuteActionInput};
use wire_types::QualifiedName;

fn main() -> anyhow::Result<()> {
    let driver = Driver::open_default()?;

    let objects = driver.list_objects(None)?;
    println!("objects: {}", objects.len());

    let action = QualifiedName::new("craft-basics", "CraftWood");
    let report = driver.check_action(&action)?;
    println!("feasible: {}", report.feasible);

    let record = driver.execute(ExecuteActionInput {
        action,
        input_objects: vec!["craft-basics__log_....dobj".to_string()],
        args: Default::default(),
    })?;
    println!("transaction {} is {}", record.tx_final, record.state);

    // Later, as often as wanted: settle marks the transaction landed or
    // expired and moves the object files.
    let outcome = driver.settle()?;
    println!("records changed: {}", outcome.records.len());
    Ok(())
}
```
