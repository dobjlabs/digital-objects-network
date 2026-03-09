# Relayer

Service that accepts zk-craft proof payloads over HTTP and relays them to Ethereum as EIP-4844 blob transactions.

## What it does

1. Accepts proof payload submissions (`POST /api/v1/proofs`).
2. Verifies payload format/proof using shared parser logic from `common`.
3. Persists jobs in RocksDB with idempotency keyed by `tx_final`.
4. Runs a single worker that submits blob transactions and polls receipts.
5. Exposes job status (`GET /api/v1/proofs/{job_id}`) and health (`GET /healthz`).

## API

- `GET /healthz` (no auth)
- `POST /api/v1/proofs` (Bearer auth)
- `GET /api/v1/proofs/{job_id}` (Bearer auth)

## Required env vars

- `RELAYER_BIND`
- `RELAYER_DB_PATH`
- `RELAYER_RPC_URL`
- `RELAYER_TO_ADDRESS`
- `RELAYER_PRIVATE_KEY`
- `RELAYER_API_KEY`

## Optional env vars

- `RELAYER_MAX_ATTEMPTS` (default: `8`)
- `RELAYER_RETRY_INITIAL_SECS` (default: `4`)
- `RELAYER_RETRY_MAX_SECS` (default: `300`)
- `RELAYER_RECEIPT_POLL_SECS` (default: `6`)
- `RELAYER_RECEIPT_TIMEOUT_SECS` (optional timeout for submitted tx receipts)
- `RELAYER_WORKER_IDLE_SLEEP_MS` (default: `1000`)
- `RELAYER_MAX_FEE_PER_BLOB_GAS` (optional override)

## Run

```bash
RUST_LOG=info cargo run -p relayer --release
```
