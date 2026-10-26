# Service models

One Quint installation supplies the service specifications. Quint Connect adapters apply external events to production Rust operations and compare observed state with the specification after every transition.

| Contract | Scope | Rust bridge |
| --- | --- | --- |
| [Synchronizer ancestry](../../services/synchronizer/model/README.md) | Fetching, skipped slots, forks, rollback and replay | Real fetch clients, derivation, RocksDB and Postgres; mocked proof parser |
| [Synchronizer publication and reads](../../services/synchronizer/model/PUBLICATION.md) | Interrupted writes, rollback and concurrent snapshots | Paused production writes and reads, SQL transactions and persisted Merkle state |
| [Relayer delivery](../../services/relayer/model/README.md) | Retry scheduling, durable broadcasts, replacements, receipts and recovery | Real worker and Postgres; independent gateway ledger and test clock |
| [Archiver acquisition](../../services/archiver/model/README.md) | Parent payload ownership, partial publication, forks and recovery | Real HTTP clients, blob selection and filesystem operations |

## Run

Requires Node.js 22, pnpm 10, and the repository's Rust toolchain and build prerequisites. From the repository root:

```sh
pnpm --dir devtools/models install --frozen-lockfile --ignore-scripts
pnpm --dir devtools/models check
```

Use `check:<service>` to check one service. Scripts typecheck specifications, execute deterministic scenarios and simulate bounded traces. Limits and seeds live in [package.json](package.json) and the Rust adapters' Quint attributes. The default Rust evaluator downloads on first use. The [Quint CI job](../../.github/workflows/models.yml) runs these checks independently of Rust compilation.

For Rust integration, start PostgreSQL with a test role that has CREATEDB permission:

```sh
export TEST_PG_ADMIN_URL=postgres://postgres:postgres@127.0.0.1:5432/postgres
pnpm --dir devtools/models integration
```

This runs the three service suites, including ignored Connect and database tests. Tests own uniquely named databases, temporary stores and fake servers. Relayer tests also accept `TEST_RELAYER_DB_ADMIN`, which takes precedence. `QUINT_SEED` selects generated traces; `QUINT_VERBOSE=1` shows replay actions and differences.

The [Rust CI job](../../.github/workflows/rust_tests.yml) runs ignored binary tests with the same workspace features and release profile as its ordinary tests, reusing that build. Cargo runs each binary from its package directory so Connect resolves specification paths. Library-only ignored tests remain excluded.

## Scope

These bounded contracts sample executions; the default checks do not perform exhaustive verification. Specifications define exact bounds, and the individual READMEs describe assumptions and exclusions. Successful database commits, WAL flushes, synchronized filesystem operations and atomic renames are trusted primitives. Process interruption does not establish power-loss behavior. Cryptographic validity, full consensus rules, unbounded history and eventual-progress guarantees are outside these contracts.

Extend a contract with a distinct transition or observable invariant and a plausible failure mode. Keep bounds small enough to explore relevant interleavings; split independent state spaces into separate contracts. Retain regression scenarios in the specification and replay them through the generic Rust adapter.
