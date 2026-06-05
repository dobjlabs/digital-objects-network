# Synchronizer

Service that tracks Digital Object blob transactions on Ethereum, derives app state, persists persistent-container data in RocksDB, stores state heads in Postgres, and serves proof-backed query APIs over HTTP.

## What it does

1. Loads config from env.
2. Starts:
   - a sync loop that processes beacon slots in order
   - an HTTP API server
3. For each slot, it:
   - reads the beacon block header/block
   - finds blob txs sent to `TO_ADDRESS`
   - fetches matching blob sidecars
   - parses and verifies `TxFinalized` payloads
   - prefetches the payloads' created-object commitments from the Postgres created index
   - opens the current Merkle containers from the `StateHead`
   - derives the next `StateHead`
4. Publishes the new slot/head and its created-index rows in one Postgres transaction, and serves state/proof APIs for clients like `app-gui`.

## State model

The synchronizer splits state into two layers:

- RocksDB stores the content-addressed POD2 Merkle backing store
  - persistent Merkle nodes
  - persistent POD2 values
- Postgres stores the control plane
  - slot metadata
  - the `StateHead` for each slot
  - the created index: object commitment -> position in the created array

There is no state head stored in RocksDB, and no resident in-memory head/cache. Every slot derivation and API read loads the current state head from Postgres, then reopens the needed persistent containers from RocksDB by root.

### `StateHead`

`StateHead` is the compact committed app-state snapshot stored on slot rows. It is split into:

- `StateRoots`
  - `created`
  - `nullifiers`
  - `state_history`
  - `next_state_history`
- `StateMetadata`
  - `current_state_root`
  - `current_block_number`
  - `created_count`
  - `nullifier_count`
  - `state_root_count`

`StateRoots.state_history` is the prior-state root array root committed inside `txlib::StateHeader`.
`StateRoots.next_state_history` is the full state root-history root after appending `StateMetadata.current_state_root`.

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
- state root history: persistent `Array`

The synchronizer reopens those containers from the roots stored in `StateRoots`.

Important: RocksDB is not the source of truth. It is an append-only backing store for Merkle data. Derivation may materialize nodes that are never committed; those orphaned nodes are harmless and are ignored unless a future `StateHead` points at them.

### Postgres (`SYNC_METADATA_DB_URL`) — state heads and sync metadata

Postgres stores the synchronization state:

- `canonical_slots`
  - one row per slot
  - on first initialization, the first row is a bootstrap row at `start_slot - 1`
  - that bootstrap row uses the real beacon/execution metadata when `start_slot - 1` had a block
  - if `start_slot - 1` was an empty beacon slot, its block fields remain `NULL`
  - includes `block_root`, `parent_root`, `execution_block_number`, `current_state_root`, `is_empty`
  - includes normalized `head_*` columns for the `StateHead` at that slot
- `created_index`
  - reverse index from object commitment to its position in the created array
  - one row per created object: `commitment` (primary key), `array_index`, `slot`
  - rows are inserted in the same transaction that commits their slot and
    deleted in the same transaction that rolls the slot back, so the index
    never diverges from the state head
  - reads treat the index as a hint: every membership and proof answer
    cross-checks it against the created array at the queried root

Postgres is the sole source of `StateHead`. The highest committed
`canonical_slots.slot` is the current state head.

## Slot derivation rules

For each decoded blob payload, the synchronizer:

- skips blobs that do not decode into a valid `TxFinalized` proof
- rejects payloads whose `state_root_hash` is not in recent state root history
- rejects payloads whose grounding state root is older than `MAX_STATE_ROOT_AGE_BLOCKS` (currently 300)
- rejects creation collisions: a created-object commitment that repeats within
  the payload, repeats within the slot, or already exists in committed state
  (prefetched from the created index and cross-checked against the created
  array). This is what gives no-input (mining) txs their replay protection.
- rejects duplicate/spent nullifiers
- appends accepted object commitments to the created array and inserts
  nullifiers into the persistent set

After processing all blobs in the slot, it computes the next state root from:

- current execution block number
- created array root
- nullifiers set root
- prior state root-array root

Then it appends that new state root to the persistent state root history array and derives a new `StateHead`.

Important: state root history advances for each processed slot, even if that slot accepted zero app transactions.

## Publish, recovery, and reorgs

### Publish

The synchronizer derives candidate state by mutating persistent containers in RocksDB. Once derivation succeeds, publication is a single Postgres transaction:

1. Insert the slot row, including the normalized `head_*` columns
2. Insert one `created_index` row per object commitment the slot added

Because the Merkle nodes were already materialized during derivation, there is no second write to RocksDB.

### Crash semantics

- If the process crashes before the Postgres commit, the old state head remains in force and any newly written RocksDB nodes are just orphaned.
- If the process crashes after the Postgres commit, the published `StateHead` is already durable and sufficient to reopen the persistent containers.

### Reorg handling

On reorg:

- the synchronizer finds the last common ancestor slot
- deletes `canonical_slots` rows after the keep-point, and the `created_index`
  rows those slots added, in one transaction
- resumes syncing from the first divergent slot

Reorg rollback does not modify RocksDB. The surviving Postgres `StateHead` determines which Merkle roots are current after rewind.

## API

- `GET /healthz`
  - returns `{"ok": true}`
- `GET /sync-progress`
  - returns `last_processed_slot`, `last_processed_block_number`
- `GET /v1/state/head`
  - returns:
    - `last_processed_slot`
    - `last_processed_block_number`
    - `current_state_root`
    - `current_block_number`
    - `created_count`
    - `nullifier_count`
    - `state_root_count`
- `POST /v1/state/membership`
  - request body:
    - `object_commitments` (array of hash strings)
    - `nullifiers` (array of hash strings)
  - returns membership for both sets from one captured state head:
    - `last_processed_slot`
    - `current_state_root`
    - `created_results` (array of `{ commitment, present }`)
    - `nullifier_results` (array of `{ nullifier, present }`)
- `POST /v1/state/object/contains`
  - request body:
    - `object_commitments` (array of hash strings)
  - returns:
    - `last_processed_slot`
    - `current_state_root`
    - `results` (array of `{ commitment, present }`)
- `POST /v1/state/nullifier/contains`
  - request body:
    - `nullifiers` (array of hash strings)
  - returns:
    - `last_processed_slot`
    - `current_state_root`
    - `results` (array of `{ nullifier, present }`)
- `POST /v1/txlib/grounding-witness`
  - request body:
    - `objectCommitments` (array of hash strings)
  - returns:
    - `stateRoot`
    - `blockNumber`
    - `createdRoot`
    - `nullifiersRoot`
    - `stateHistoryRoot`
    - `createdProofs` (array of `{ commitment, present, index, proof }`)

Membership and grounding-witness requests read the state head and the
created index from one `REPEATABLE READ` Postgres transaction, so the index
entries always match the captured head's roots. The RocksDB reads then run
against those pinned roots; Merkle nodes are content-addressed and immutable
by root, so a concurrent commit or rollback cannot change what the captured
roots resolve to.

Hashes in request and response bodies are pod2 `Hash` values, serialized as lowercase 64-character hex with no `0x` prefix; inputs must use that same form (a `0x` prefix or wrong length is rejected).

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
