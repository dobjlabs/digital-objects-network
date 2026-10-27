# Transaction record contract

[transactions.qnt](transactions.qnt) models the driver's transaction lifecycle: proof holds, recording, submission attempts and retries, settlement queries and their application, recoverable write failures, and process crashes. [transactions.test.qnt](transactions.test.qnt) exercises deterministic boundary cases; `safety` is checked by random simulation over the same transitions.

Local state includes object files, payload files, transaction records, proof jobs, holds, and a pending settlement snapshot. Objects, payloads, and records survive a restart. Jobs, holds, and snapshots do not. Each record has a durable `attempted` flag, representing the presence of `submitAttemptedAtMs`; the model does not track timestamps. The environment is the chain and one relayer job per transaction ID. Small integers stand for filenames, commitments, nullifiers, and transaction IDs.

## What the contract says

Open records (`Recorded`, `Submitted`) have disjoint inputs, and no proof holds an input owned by an open record. `Landed` and `Expired` are terminal and agree with actual chain inclusion. A landed record retains its outputs with confirmed statuses and its inputs are nullified. An expired record owns no unknown output. Unknown files belong to open records, except outputs left by a crash before recording committed. File status moves only forward, and a surviving file cannot change producer.

Submission attempts are durable and cannot be undone by failure or restart while the transaction is open. Deletion uses that local flag: it is forbidden after an attempt even when the relayer has no job. A successful request records acceptance; an unavailable relayer leaves the record attempted but unaccepted; a lost response can leave a remote job without local acceptance. Retrying reuses that job. Automatic retries require an attempted, still-valid `Recorded` transaction. They cannot submit untouched records or retry terminal transactions.

Settlement has separate `querySettlement` and `applySettlement` transitions. A query captures the transaction IDs, the subset with submission attempts that require membership, requested membership results and their head, and relayer annotations. Recording, submission, deletion, chain progress, and crashes can occur before application. Application changes only records in that snapshot, using its head for expiry; a late record waits for another query. Never-submitted records use head-only expiry. If a record acquires an attempt while such a query is pending, it stays open until a later query requests its membership. Only one query can be pending, and restart discards it. `settle` is a convenience action that captures and applies a snapshot atomically for lifecycle scenarios; random simulation uses the split transitions.

`recordFailed` models recoverable failures during output, payload, and record writes. It can write a subset of outputs before an output-write failure, then rolls back its files and releases only its own proof holds. Other jobs and existing object files survive. Crashes remain separate: outputs or payloads written before the record's commit point can remain on disk without a transaction record.

`prune` removes only terminal records and payloads, preserving every object file. The model keeps `prunedRecords` solely as verification history, so chain agreement and output preservation remain checked after the local receipt disappears. This is not a runtime archive. Retention timing and Ethereum hash metadata are covered by Rust tests; the model permits pruning at any point after settlement. The implementation also replays durable terminal records to repair incomplete local file updates before pruning.

## Boundary scenarios

The deterministic suite includes:

- Failure before relayer acceptance, restart, repeated failure, and successful retry; lost responses reuse a relayer job without clearing its annotations.
- Refused deletion after an unsuccessful submission attempt, and refused automatic retries for untouched, stale, landed, and expired transactions.
- Partial output rollback while another proof holds unrelated inputs, plus payload and record-write rollback followed by reuse of released inputs and output names.
- A stale proof recorded between settlement query and application, deletion followed by a replacement record, and a landing at the grounding-age limit while an earlier query is pending.
- Discarding a pending settlement snapshot on restart and refusing overlapping queries.
- Separating membership checks from head-only expiry in mixed batches, and preserving a concurrent submission until membership is queried.
- Pruning landed and expired receipts, restarting and spending a retained object, and refusing to prune either kind of open transaction.

The earlier ownership, collision, chained-spend, expiry, and crash scenarios remain. `relayerFailureIsAnAnnotationTest` checks that a failed relayer job may still land; `crashAfterRelayerAcceptStillSettlesTest` checks landing without locally recorded acceptance.

## Assumptions and exclusions

The chain accepts a payload only while `head - grounding <= maxAge` and only when its commitments and nullifiers do not collide with committed state. The real rule compares the inclusion block with grounding, so the model admits landings the synchronizer would refuse, which is conservative for safety.

A query captures membership and head together. It includes the current object files as well as open-record effects, representing an inventory reconciliation pass. Multi-batch HTTP behavior is abstracted into that snapshot. The record commit and each crash-phase output batch are atomic; torn files and crashes during partial output batches are excluded. Recoverable write failures and their rollback form one transition under the store lock, and rollback filesystem operations are assumed to succeed. Cleanup failures and corrupted files are outside this model.

Imports, unsafe discard, externally proved transactions, explicit reservations, and relayer fee bumps remain excluded. GUI events, run progress, exact retry delays, and backoff bookkeeping are covered by implementation tests. Simulation has no fairness requirement and makes no eventual-completion claim: the relayer or chain may stop progressing indefinitely.

Bounds: four object IDs, two initially live objects, at most four proof starts, a head of at most eight blocks, and a grounding age limit of two. Simulation explores submission outcomes, recoverable failures, query/application interleavings, and crashes alongside the normal lifecycle. Deterministic scenarios cover cases that sampling may rarely reach.

## Run

Requires Node.js 22 and pnpm 10.34.8. From the repository root:

```sh
just models
```

For individual checks, install the pinned tools once:

```sh
pnpm --dir devtools/models install --frozen-lockfile --ignore-scripts
pnpm --dir devtools/models typecheck:driver
pnpm --dir devtools/models test:driver
pnpm --dir devtools/models simulate:driver
```

`check:driver` runs all three. The [shared tooling](../../../devtools/models/README.md) pins Quint 0.33.0, the dependency lockfile, and the simulation seed and bounds. The [Quint workflow](../../../.github/workflows/models.yml) runs these checks in CI.

## Rust replay adapter

The driver adapter is still to be implemented. It will replay transitions against a `Driver` on a temporary home, with mock synchronizer and relayer state, controlled query and crash boundaries, and restart by reopening the driver. It will compare files, records, and holds with `node`. `command`, `deleted`, `strays`, `previousObjects`, `previousRecords`, and `prunedRecords` support trace inspection and specification checks; they are not persisted driver data. Passing the model checks validates the specification, without proving that Rust implements it.
