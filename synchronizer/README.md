# Synchronizer

Service that tracks Digital Object blob transactions on Ethereum and exposes current derived state over HTTP.

## What it does

1. Loads config from env.
2. Starts:
   - a sync loop that processes beacon slots in order
   - an HTTP API server
3. For each slot, it:
   - reads the beacon block header/block
   - finds blob txs sent to `TO_ADDRESS`
   - fetches matching blob sidecars
   - decodes payload bytes and derives new state
4. Persists app state in RocksDB and sync metadata in Postgres, and serves sync/state query APIs over HTTP.

## Storage model

### Postgres (`SYNC_METADATA_DB_URL`) — sync control plane

Postgres stores synchronizer metadata and slot-level apply/rollback journaling:

- `sync_cursor`
  - single-row progress cursor (`last_processed_slot`, `last_processed_block_number`)
- `canonical_slots`
  - canonical beacon/execution metadata per slot (`slot`, `block_root`, `parent_root`, `execution_block_number`, `is_empty`, `status`)
- `slot_apply_journal`
  - per-slot KV delta and lifecycle (`tx_hashes`, `nullifiers`, `gsr_block_numbers`, `gsr_hashes`, `op`, `kv_applied`)

This is used for deterministic reorg handling and crash-safe recovery.

### RocksDB (`APP_STATE_DB_PATH`) — app-derived state store

RocksDB stores only app-derived state:

- accepted transaction hashes
- spent nullifiers
- global state roots (GSRs)

RocksDB is updated from Postgres journaled slot deltas and rolled back using the same journal data.

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
- `GET /v1/state/full`
  - returns:
    - `block_number`
    - `current_gsr`
    - `transactions` (array of tx hashes)
    - `nullifiers` (array of nullifier hashes)
    - `gsrs` (array of prior GSR hashes)
- `POST /v1/state/tx/contains`
  - request body:
    - `tx_hashes` (array of hash strings)
  - returns:
    - `last_processed_slot`
    - `current_gsr`
    - `results` (array of `{ tx_hash, present }`)
- `GET /v1/state/tx/{tx_hash}`
  - returns:
    - `tx_hash`
    - `present`
    - `last_processed_slot`
    - `current_gsr`
- `GET /v1/dashboard/summary`
  - returns:
    - `status`
    - `status_reason`
    - `last_processed_slot`
    - `beacon_head_slot`
    - `slot_lag`
    - `last_processed_block_number`
    - `current_block_number`
    - `current_gsr`
    - `tx_count`
    - `nullifier_count`
    - `gsr_count`
    - `pending_recovery_count`
    - `cursor_updated_at`
- `GET /v1/dashboard/recent-slots?limit=25`
  - returns:
    - `slots` array of:
      - `slot`
      - `execution_block_number`
      - `status`
      - `is_empty`
      - `block_root`
      - `parent_root`
      - `tx_count`
      - `nullifier_count`
      - `gsr_hash`
      - `updated_at`

Hash parsing accepts `0x`-prefixed or raw hex input; responses are normalized to lowercase `0x...`.

## Required env vars

- `RPC_URL`
- `BEACON_URL`
- `TO_ADDRESS`

## Optional env vars

- `APP_STATE_DB_PATH` (default: `data/synchronizer-db`)
- `SYNC_METADATA_DB_URL` (default: `postgres://postgres@localhost:5432/synchronizer`)
- `HTTP_BIND` (default: `127.0.0.1:3000`)
- `CORS_ALLOWED_ORIGINS` (default: unset; comma-separated browser origins allowed to call the API)
- `SYNC_DELAY_MS` (default: `333`)
- `INITIAL_START_SLOT` (default: unset, meaning start from current head on first run)

## Run

```bash
RUST_LOG=info cargo run --release --bin synchronizer
```
