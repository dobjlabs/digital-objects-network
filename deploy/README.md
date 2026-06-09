# Run the synchronizer and relayer

Both services ship as published container images:

- `ghcr.io/dobjlabs/digital-objects-network/synchronizer`
- `ghcr.io/dobjlabs/digital-objects-network/relayer`

Run the full stack, or just a **synchronizer** to independently verify the
network's canonical state from Ethereum chain data without trusting anyone
else's instance.

## Quick start

```bash
cp deploy/.env.example deploy/.env   # fill in your endpoints
docker compose -f deploy/compose.yaml up -d
```

This brings up Postgres plus both services. Check them:

```bash
curl localhost:3000/healthz         # synchronizer up
curl localhost:3000/sync-progress   # is it following chain head?
curl localhost:3200/healthz         # relayer up
```

(Requires the published images. If they aren't published yet, build them
locally first - see "Building the images yourself" below.)

### Just a synchronizer (verify-only)

A synchronizer needs only an execution RPC, a beacon endpoint, and Postgres -
no wallet key. Run only those services:

```bash
docker compose -f deploy/compose.yaml up -d synchronizer postgres
```

Or fully standalone against your own Postgres, no compose. Create the
`synchronizer` database first - the service creates its tables, but not the
database:

```bash
psql "postgres://user:pass@host:5432/postgres" -c 'CREATE DATABASE synchronizer'

docker run -d --name synchronizer -p 3000:3000 \
  -e RPC_URL=https://your-execution-rpc \
  -e BEACON_URL=https://your-beacon-api \
  -e TO_ADDRESS=0x... \
  -e SYNC_METADATA_DB_URL=postgres://user:pass@host:5432/synchronizer \
  -v don_data:/var/lib/don \
  ghcr.io/dobjlabs/digital-objects-network/synchronizer:latest
```

## Configuration

Set via `.env` (compose) or `-e` flags (`docker run`). Image defaults:
`HTTP_BIND=0.0.0.0:<port>`, and the synchronizer's
`APP_STATE_DB_PATH=/var/lib/don/synchronizer-db`.

| Variable               | Service      | Required     | Notes                                                             |
| ---------------------- | ------------ | ------------ | ----------------------------------------------------------------- |
| `RPC_URL`              | both         | yes          | Execution-layer RPC                                               |
| `BEACON_URL`           | synchronizer | yes          | Beacon API with blob sidecars                                     |
| `TO_ADDRESS`           | both         | yes          | L1 target address; must match across both                         |
| `PRIVATE_KEY`          | relayer      | relayer only | Hot wallet that signs/pays for blob txs                           |
| `SYNC_METADATA_DB_URL` | synchronizer | no           | Defaults to the bundled Postgres; set to point at an external one |
| `DB_URL`               | relayer      | no           | Defaults to the bundled Postgres; set to point at an external one |
| `IMAGE_TAG`            | compose      | no           | Image tag to run; pin to a release                                |
| `APP_STATE_DB_PATH`    | synchronizer | no           | RocksDB cache path; mount a volume here                           |
| `HTTP_BIND`            | both         | no           | Defaults to `0.0.0.0:3000` / `0.0.0.0:3200`                       |
| `RUST_LOG`             | both         | no           | e.g. `info`                                                       |

The synchronizer's `/var/lib/don` holds RocksDB - a rebuildable cache (the
authoritative state lives in Postgres), but mounting a volume avoids a slow cold
re-sync on restart. The relayer keeps no local state.

## Operational notes

- Both services are **singletons**. Never run two of either against the same
  database (the relayer collides on nonces; the synchronizer on slot commits).
- **Pin `IMAGE_TAG`** to a release (e.g. `v0.1.0`) for reproducible runs.
- **Hardened production:** point `SYNC_METADATA_DB_URL` / `DB_URL` at a managed
  Postgres (the synchronizer's state is authoritative - back it up) and inject
  `PRIVATE_KEY` from a secret store instead of a plaintext `.env`. The relayer
  has no runtime balance check, so monitor the wallet balance externally.

## Building the images yourself

The compose pulls published images. To build from source instead, tag them with
the names the compose expects and point `IMAGE_TAG` at your tag:

```bash
docker build --target synchronizer -t ghcr.io/dobjlabs/digital-objects-network/synchronizer:dev .
docker build --target relayer      -t ghcr.io/dobjlabs/digital-objects-network/relayer:dev .
# then set IMAGE_TAG=dev in deploy/.env
```
