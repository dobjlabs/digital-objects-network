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
   - opens the current Merkle containers from the canonical `CanonicalHead`
   - derives the next `CanonicalHead`
4. Publishes the new canonical slot/head in Postgres and serves state/proof APIs for clients like `app-gui`.

## State model

The synchronizer splits state into two layers:

- RocksDB stores the content-addressed POD2 Merkle backing store
  - persistent Merkle nodes
  - persistent POD2 values
- Postgres stores the canonical control plane
  - the sync cursor
  - canonical slot metadata
  - the canonical `CanonicalHead` for each slot

There is no canonical head stored in RocksDB, and no resident in-memory head/cache. Every slot derivation and API read loads the current canonical head from Postgres, then reopens the needed persistent containers from RocksDB by root.

### `CanonicalHead`

`CanonicalHead` is the compact committed app-state snapshot stored on canonical slot rows. It is split into:

- `CanonicalRoots`
  - `transactions`
  - `nullifiers`
  - `state_root_gsrs`
  - `gsr_history`
- `HeadMetadata`
  - `current_gsr`
  - `current_block_number`
  - `tx_count`
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

- transactions: persistent `Set`
- nullifiers: persistent `Set`
- GSR history: persistent `Array`

The synchronizer reopens those containers from the roots stored in `CanonicalRoots`.

Important: RocksDB is not the source of canonical truth. It is an append-only backing store for Merkle data. Derivation may materialize nodes that never become canonical; those orphaned nodes are harmless and are ignored unless a future `CanonicalHead` points at them.

### Postgres (`SYNC_METADATA_DB_URL`) — canonical heads and sync metadata

Postgres stores the canonical synchronization state:

- `sync_cursor`
  - single-row progress cursor (`last_processed_slot`, `last_processed_block_number`)
- `canonical_slots`
  - one row per canonical slot
  - includes `block_root`, `parent_root`, `execution_block_number`, `current_gsr`, `is_empty`
  - includes normalized `head_*` columns for the canonical `CanonicalHead` at that slot

Postgres is the sole source of canonical `CanonicalHead`.

## Slot derivation rules

For each decoded blob payload, the synchronizer:

- skips blobs that do not decode into a valid `TxFinalized` proof
- rejects payloads whose `state_root_hash` is not in recent canonical GSR history
- rejects payloads whose grounding GSR is older than `MAX_GSR_AGE_BLOCKS` (currently 300)
- rejects duplicate `tx_final`
- rejects duplicate/spent nullifiers
- inserts accepted txs/nullifiers into the persistent sets

After processing all blobs in the slot, it computes the next GSR from:

- current execution block number
- transactions set root
- nullifiers set root
- prior GSR-array root

Then it appends that new GSR to the persistent GSR history array and derives a new `CanonicalHead`.

Important: GSR history advances for each canonical processed slot, even if that slot accepted zero app transactions.

## Publish, recovery, and reorgs

### Canonical publish

The synchronizer derives candidate state by mutating persistent containers in RocksDB. Once derivation succeeds, canonical publication is a single Postgres transaction:

1. Insert or update the canonical slot row, including the normalized `head_*` columns
2. Advance `sync_cursor` to that slot

Because the Merkle nodes were already materialized during derivation, there is no second canonical write to RocksDB.

### Crash semantics

- If the process crashes before the Postgres commit, the old canonical head remains in force and any newly written RocksDB nodes are just orphaned.
- If the process crashes after the Postgres commit, the published `CanonicalHead` is already durable and sufficient to reopen the canonical containers.

### Reorg handling

On reorg:

- the synchronizer finds the last common ancestor slot
- deletes `canonical_slots` rows after the keep-point
- rewinds `sync_cursor` in the same Postgres transaction
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
    - `tx_count`
    - `nullifier_count`
    - `gsr_count`
- `POST /v1/state/membership`
  - request body:
    - `tx_hashes` (array of hash strings)
    - `nullifiers` (array of hash strings)
  - returns membership for both sets from one captured canonical head:
    - `last_processed_slot`
    - `current_gsr`
    - `tx_results` (array of `{ tx_hash, present }`)
    - `nullifier_results` (array of `{ nullifier, present }`)
- `POST /v1/state/tx/contains`
  - request body:
    - `tx_hashes` (array of hash strings)
  - returns:
    - `last_processed_slot`
    - `current_gsr`
    - `results` (array of `{ tx_hash, present }`)
- `POST /v1/state/nullifier/contains`
  - request body:
    - `nullifiers` (array of hash strings)
  - returns:
    - `last_processed_slot`
    - `current_gsr`
    - `results` (array of `{ nullifier, present }`)
- `GET /v1/state/tx/{tx_hash}`
  - returns:
    - `tx_hash`
    - `present`
    - `last_processed_slot`
    - `current_gsr`
- `POST /v1/txlib/grounding-witness`
  - request body:
    - `sourceTxHashes` (array of hash strings)
  - returns:
    - `stateRootHash`
    - `blockNumber`
    - `transactionsRoot`
    - `nullifiersRoot`
    - `gsrsRoot`
    - `sourceTxProofs` (array of `{ txHash, present, proof }`)

Each request captures one current Postgres snapshot, then uses that exact `CanonicalHead` for all RocksDB membership/proof reads in the response.

Hash parsing accepts `0x`-prefixed or raw hex input; responses are normalized to lowercase `0x...`.

## Required env vars

- `RPC_URL`
- `BEACON_URL`
- `TO_ADDRESS`

## Optional env vars

- `APP_STATE_DB_PATH` (default: `data/synchronizer-db`)
- `SYNC_METADATA_DB_URL` (default: `postgres://postgres@localhost:5432/synchronizer`)
- `HTTP_BIND` (default: `127.0.0.1:3000`)
- `SYNC_DELAY_MS` (default: `333`)
- `INITIAL_START_SLOT` (default: unset, meaning start from current head on first run)

## Run

```bash
RUST_LOG=info cargo run --release --bin synchronizer
```
