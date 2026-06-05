# Synchronizer

Service that tracks Digital Object blob transactions on Ethereum, derives canonical app state, persists persistent-container data in RocksDB, stores canonical heads in Postgres, and serves proof-backed query APIs over HTTP.

## What it does

1. Loads config from env.
2. Starts:
   - a sync loop that processes beacon slots in order
   - an HTTP API server
3. For each canonical slot, it:
   - reads the beacon block header/block
   - finds blob txs sent to `TO_ADDRESS`
   - fetches matching blob sidecars
   - parses and verifies `TxFinalized` payloads
   - prefetches the payloads' created-object commitments from the Postgres created index
   - opens the current Merkle containers from the canonical `CanonicalHead`
   - derives the next `CanonicalHead`
4. Publishes the new canonical slot/head and its created-index rows in one Postgres transaction, and serves state/proof APIs for clients like `app-gui`.

## State model

The synchronizer splits state into two layers:

- RocksDB stores the content-addressed POD2 Merkle backing store
  - persistent Merkle nodes
  - persistent POD2 values
- Postgres stores the canonical control plane
  - canonical slot metadata
  - the canonical `CanonicalHead` for each slot
  - the created index: object commitment -> position in the created array

There is no canonical head stored in RocksDB, and no resident in-memory head/cache. Every slot derivation and API read loads the current canonical head from Postgres, then reopens the needed persistent containers from RocksDB by root.

### `CanonicalHead`

`CanonicalHead` is the compact committed app-state snapshot stored on canonical slot rows. It is split into:

- `CanonicalRoots`
  - `created`
  - `nullifiers`
  - `state_root_gsrs`
  - `gsr_history`
- `HeadMetadata`
  - `current_gsr`
  - `current_block_number`
  - `created_count`
  - `nullifier_count`
  - `gsr_count`

`CanonicalRoots.state_root_gsrs` is the prior-GSR array root committed inside `txlib::StateRoot`.
`CanonicalRoots.gsr_history` is the full GSR-history root after appending `HeadMetadata.current_gsr`.

## Storage model

### RocksDB (`APP_STATE_DB_PATH`) — persistent Merkle backing store

RocksDB stores:

- `n/...`
  - persistent Merkle nodes
- `v/...`
  - persistent POD2 values

The Merkle-backed containers are:

- created objects: persistent `Array` of object commitments (0-indexed)
- nullifiers: persistent `Set`
- GSR history: persistent `Array`

The synchronizer reopens those containers from the roots stored in `CanonicalRoots`.

Important: RocksDB is not the source of canonical truth. It is an append-only backing store for Merkle data. Derivation may materialize nodes that never become canonical; those orphaned nodes are harmless and are ignored unless a future `CanonicalHead` points at them.

### Postgres (`SYNC_METADATA_DB_URL`) — canonical heads and sync metadata

Postgres stores the canonical synchronization state:

- `canonical_slots`
  - one row per canonical slot
  - on first initialization, the first row is a bootstrap row at `start_slot - 1`
  - that bootstrap row uses the real beacon/execution metadata when `start_slot - 1` had a block
  - if `start_slot - 1` was an empty beacon slot, its block fields remain `NULL`
  - includes `block_root`, `parent_root`, `execution_block_number`, `current_gsr`, `is_empty`
  - includes normalized `head_*` columns for the canonical `CanonicalHead` at that slot
- `created_index`
  - reverse index from object commitment to its position in the created array
  - one row per created object: `commitment` (primary key), `array_index`, `slot`
  - rows are inserted in the same transaction that commits their slot and
    deleted in the same transaction that rolls the slot back, so the index
    never diverges from the canonical head
  - reads treat the index as a hint: every membership and proof answer
    cross-checks it against the created array at the queried root

Postgres is the sole source of canonical `CanonicalHead`. The highest committed
`canonical_slots.slot` is the current canonical head.

## Slot derivation rules

For each decoded blob payload, the synchronizer:

- skips blobs that do not decode into a valid `TxFinalized` proof
- rejects payloads whose `state_root_hash` is not in recent canonical GSR history
- rejects payloads whose grounding GSR is older than `MAX_GSR_AGE_BLOCKS` (currently 300)
- rejects creation collisions: a created-object commitment that repeats within
  the payload, repeats within the slot, or already exists in committed state
  (prefetched from the created index and cross-checked against the created
  array). This is what gives no-input (mining) txs their replay protection.
- rejects duplicate/spent nullifiers
- appends accepted object commitments to the created array and inserts
  nullifiers into the persistent set

After processing all blobs in the slot, it computes the next GSR from:

- current execution block number
- created array root
- nullifiers set root
- prior GSR-array root

Then it appends that new GSR to the persistent GSR history array and derives a new `CanonicalHead`.

Important: GSR history advances for each canonical processed slot, even if that slot accepted zero app transactions.

## Publish, recovery, and reorgs

### Canonical publish

The synchronizer derives candidate state by mutating persistent containers in RocksDB. Once derivation succeeds, canonical publication is a single Postgres transaction:

1. Insert the canonical slot row, including the normalized `head_*` columns
2. Insert one `created_index` row per object commitment the slot added

Because the Merkle nodes were already materialized during derivation, there is no second canonical write to RocksDB.

### Crash semantics

- If the process crashes before the Postgres commit, the old canonical head remains in force and any newly written RocksDB nodes are just orphaned.
- If the process crashes after the Postgres commit, the published `CanonicalHead` is already durable and sufficient to reopen the canonical containers.

### Reorg handling

On reorg:

- the synchronizer finds the last common ancestor slot
- deletes `canonical_slots` rows after the keep-point, and the `created_index`
  rows those slots added, in one transaction
- resumes syncing from the first divergent slot

Reorg rollback does not modify RocksDB. The surviving Postgres `CanonicalHead` determines which Merkle roots are canonical after rewind.

## API

- `GET /healthz`
  - returns `{"ok": true}`
- `GET /sync-progress`
  - returns `last_processed_slot`, `last_processed_block_number`
- `GET /v1/state/head`
  - returns:
    - `last_processed_slot`
    - `last_processed_block_number`
    - `current_gsr`
    - `current_block_number`
    - `created_count`
    - `nullifier_count`
    - `gsr_count`
- `POST /v1/state/membership`
  - request body:
    - `object_commitments` (array of hash strings)
    - `nullifiers` (array of hash strings)
  - returns membership for both sets from one captured canonical head:
    - `last_processed_slot`
    - `current_gsr`
    - `created_results` (array of `{ commitment, present }`)
    - `nullifier_results` (array of `{ nullifier, present }`)
- `POST /v1/state/object/contains`
  - request body:
    - `object_commitments` (array of hash strings)
  - returns:
    - `last_processed_slot`
    - `current_gsr`
    - `results` (array of `{ commitment, present }`)
- `POST /v1/state/nullifier/contains`
  - request body:
    - `nullifiers` (array of hash strings)
  - returns:
    - `last_processed_slot`
    - `current_gsr`
    - `results` (array of `{ nullifier, present }`)
- `POST /v1/txlib/grounding-witness`
  - request body:
    - `objectCommitments` (array of hash strings)
  - returns:
    - `stateRootHash`
    - `blockNumber`
    - `createdRoot`
    - `nullifiersRoot`
    - `gsrsRoot`
    - `createdProofs` (array of `{ commitment, present, index, proof }`)

Membership and grounding-witness requests read the canonical head and the
created index from one `REPEATABLE READ` Postgres transaction, so the index
entries always match the captured head's roots. The RocksDB reads then run
against those pinned roots; Merkle nodes are content-addressed and immutable
by root, so a concurrent commit or rollback cannot change what the captured
roots resolve to.

Hash parsing accepts `0x`-prefixed or raw hex input; responses are normalized to lowercase `0x...`.

## Required env vars

- `RPC_URL`
- `BEACON_URL`
- `TO_ADDRESS`

## Optional env vars

- `APP_STATE_DB_PATH` (default: `data/synchronizer-db`)
- `SYNC_METADATA_DB_URL` (default: `postgres://postgres@localhost:5432/synchronizer`)
- `HTTP_BIND` (default: `127.0.0.1:3000`)
- `RPC_RETRIES` (default: `6`)
- `RPC_RETRY_MS` (default: `1000`)
- `INITIAL_START_SLOT` (default: unset, meaning start from current head on first run)
- `SYNC_DELAY_MS` (default: derived from RPC rate limit, currently `173`) — delay in ms between slots when at head. Override with caution to avoid exceeding the RPC rate limit.
- `CATCHUP_BATCH_SIZE` (default: derived from RPC rate limit, currently `7`) — number of slots fetched concurrently during catch-up. Override with caution to avoid exceeding the RPC rate limit.

The defaults for `SYNC_DELAY_MS` and `CATCHUP_BATCH_SIZE` are computed from the known RPC rate limit (15 req/s) to stay safely under the limit. If either is set via env, a warning is logged as a reminder to check rate limit compliance.

## Run

```bash
RUST_LOG=info cargo run --release --bin synchronizer
```
