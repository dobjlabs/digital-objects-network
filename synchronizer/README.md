# Synchronizer

Service that tracks Digital Object blob transactions on Ethereum, derives canonical app state, persists that state in RocksDB/Postgres, and serves proof-backed query APIs over HTTP.

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
   - derives the next app-state head
4. Persists the new head crash-safely and serves state/proof APIs for clients like `app-gui`.

## State model

The synchronizer keeps canonical app state across RocksDB, Postgres, and memory:

- in RocksDB:
  - persistent POD2 Merkle container data
  - a compact `meta/head` snapshot (`AppHead`)
- in Postgres:
  - sync cursor
  - canonical slot metadata
  - apply/rollback journals
- in memory:
  - the current `AppHead`
  - a recent-GSR cache used to validate grounding roots

### `AppHead`

`AppHead` is the committed app-state snapshot:

- `transactions_root`
- `nullifiers_root`
- `state_root_gsrs_root`
- `gsr_history_root`
- `current_gsr`
- `current_block_number`
- `tx_count`
- `nullifier_count`
- `gsr_count`

`state_root_gsrs_root` is the prior-GSR array root used inside `txlib::StateRoot`.
`gsr_history_root` is the array root after appending the current GSR.

## Storage model

### RocksDB (`APP_STATE_DB_PATH`) — app state

RocksDB stores:

- `meta/head`
  - JSON-serialized `AppHead`
- `n/...`
  - persistent Merkle nodes
- `v/...`
  - persistent POD2 values

The Merkle-backed containers are:

- transactions: persistent `Set`
- nullifiers: persistent `Set`
- GSR history: persistent `Array`

The synchronizer reopens those containers from the roots stored in `AppHead`.

### Postgres (`SYNC_METADATA_DB_URL`) — sync control plane

Postgres stores synchronizer metadata and slot-level journaling:

- `sync_cursor`
  - single-row progress cursor (`last_processed_slot`, `last_processed_block_number`)
- `canonical_slots`
  - canonical slot metadata
  - includes `block_root`, `parent_root`, `execution_block_number`, `current_gsr`, `is_empty`, `status`
- `slot_apply_journal`
  - per-slot `{ old_head, new_head }` journal
  - includes `op` (`apply` or `rollback`) and `kv_applied`

This is the source of truth for crash recovery and reorg handling.

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

Then it appends that new GSR to the persistent GSR history array and stores a new `AppHead`.

Important: GSR history advances for each canonical processed slot, even if that slot accepted zero app transactions.

## Recovery and reorgs

Canonical head advancement uses a staged head-swap pipeline:

1. Save canonical slot metadata and `{ old_head, new_head }` journal in Postgres as `pending`
2. Apply `new_head` to RocksDB
3. Mark the journal/slot `applied` and advance `sync_cursor`
4. Update in-memory head/cache

On startup, the synchronizer replays any unfinished apply/rollback work from Postgres.

On reorg:

- later slots are staged as rollback entries in Postgres
- RocksDB is rewound by restoring the appropriate `old_head`
- canonical slot rows past the keep-point are removed
- recent GSR cache is rebuilt from canonical Postgres rows

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
  - returns membership for both sets from one captured application head:
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
