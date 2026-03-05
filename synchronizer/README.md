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
4. Persists app state in RocksDB and sync metadata in Postgres, and serves state at `/state`.

## API

- `GET /state`
  - returns `transactions`, `nullifiers`, `last_processed_slot`, `last_processed_block_number`

## Required env vars

- `RPC_URL`
- `BEACON_URL`
- `TO_ADDRESS`

## Optional env vars

- `APP_STATE_DB` (default: `data/synchronizer-db`)
- `SYNC_METADATA_DB` (default: `postgres://postgres@localhost:5432/synchronizer`)
- `HTTP_BIND` (default: `127.0.0.1:3000`)
- `SYNC_DELAY_MS` (default: `333`)
- `INITIAL_START_SLOT` (default: unset, meaning start from current head on first run)

## Run

```bash
RUST_LOG=info cargo run --release --bin synchronizer
```
