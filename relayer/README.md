# Relayer

Service that accepts zk-craft proof payloads over HTTP and relays them to Ethereum as EIP-4844 blob transactions.

## What it does

1. Accepts proof payload submissions (`POST /api/v1/proofs`).
2. Verifies payload format/proof using shared parser logic from `common`.
3. Persists relay jobs in Postgres with idempotency keyed by `tx_final`.
4. Runs a single worker that submits blob transactions and polls receipts.
5. Exposes job status (`GET /api/v1/proofs/{job_id}`) and health (`GET /healthz`).

## Storage model

### Postgres (`DB_URL`) — relay job queue and state

`relay_jobs` relation:

- `job_id TEXT PRIMARY KEY`
- `status TEXT NOT NULL CHECK (status IN ('queued','sending','submitted','confirmed','failed'))`
- `payload_bytes BYTEA NOT NULL`
- `tx_final TEXT NOT NULL UNIQUE`
- `state_root_hash TEXT NOT NULL`
- `client_ref TEXT NULL`
- `attempt_count INTEGER NOT NULL`
- `tx_hash TEXT NULL`
- `submitted_at BIGINT NULL`
- `block_number BIGINT NULL`
- `last_error TEXT NULL`
- `next_attempt_at BIGINT NULL`
- `created_at BIGINT NOT NULL`
- `updated_at BIGINT NOT NULL`

Indexes:

- unique index on `tx_final` (idempotent submit)
- `(status, next_attempt_at, created_at)` for due-job scheduling
- `(next_attempt_at, created_at)` for due-job ordering in non-terminal statuses

## API

- `GET /healthz` (no auth)
- `POST /api/v1/proofs` (no auth)
- `GET /api/v1/proofs/{job_id}` (no auth)

## Required env vars

- `RPC_URL`
- `TO_ADDRESS`
- `PRIVATE_KEY`

## Optional env vars

- `HTTP_BIND` (default: `127.0.0.1:3200`)
- `DB_URL` (default: `postgres://postgres@localhost:5432/relayer`)
- `MAX_ATTEMPTS` (default: `8`)
- `RETRY_INITIAL_SECS` (default: `4`)
- `RETRY_MAX_SECS` (default: `300`)
- `RECEIPT_POLL_SECS` (default: `6`)
- `RECEIPT_TIMEOUT_SECS` (optional timeout for submitted tx receipts)
- `WORKER_IDLE_SLEEP_MS` (default: `1000`)
- `MAX_FEE_PER_BLOB_GAS` (optional override)

## Run

```bash
RUST_LOG=info cargo run -p relayer --release
```
