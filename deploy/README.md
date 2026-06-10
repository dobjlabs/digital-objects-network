# Run the synchronizer, relayer, and archiver

All three services ship as published container images:

- `ghcr.io/dobjlabs/digital-objects-network/synchronizer`
- `ghcr.io/dobjlabs/digital-objects-network/relayer`
- `ghcr.io/dobjlabs/digital-objects-network/archiver`

Run the full stack, or just a **synchronizer** (with its **archiver**) to
independently verify the network's canonical state from Ethereum chain data
without trusting anyone else's instance.

## Quick start

```bash
cp deploy/.env.example deploy/.env   # fill in your endpoints
docker compose -f deploy/compose.yaml up -d
```

This brings up Postgres plus all three services. Check them:

```bash
curl localhost:3000/healthz         # synchronizer up
curl localhost:3000/sync-progress   # is it following chain head?
curl localhost:3200/healthz         # relayer up
curl localhost:3001/healthz         # archiver up
```

(Requires the published images. If they aren't published yet, build them
locally first - see "Building the images yourself" below.)

### Just a synchronizer (verify-only)

A synchronizer needs an execution RPC, a beacon endpoint, and Postgres - no
wallet key. With just those it reads blobs from the beacon (recent blobs only).
To sync history older than the beacon's ~18-day retention, also run an
**archiver** and point `ARCHIVER_URL` at it. The bundled compose includes one:

```bash
docker compose -f deploy/compose.yaml up -d synchronizer postgres
```

Or fully standalone against your own Postgres, no compose. Create the
`synchronizer` database first - the service creates its tables, but not the
database. `ARCHIVER_URL` is optional: unset, the synchronizer reads blobs from
`BEACON_URL` (recent blobs only); set it to an archiver for older history:

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

To run an archiver standalone, give it the same target address (as
`FILTER_ADDRESS`), a start slot, and a volume for its on-disk blob store:

```bash
docker run -d --name archiver -p 3001:3001 \
  -e RPC_URL=https://your-execution-rpc \
  -e BEACON_URL=https://your-beacon-api \
  -e FILTER_ADDRESS=0x... \
  -e INIT_START_SLOT=10413441 \
  -v don_blobs:/var/lib/don \
  ghcr.io/dobjlabs/digital-objects-network/archiver:latest
```

## Configuration

Set via `.env` (compose) or `-e` flags (`docker run`). Image defaults:
`HTTP_BIND=0.0.0.0:<port>`, and the synchronizer's
`APP_STATE_DB_PATH=/var/lib/don/synchronizer-db`.

| Variable               | Service                | Required     | Notes                                                                           |
| ---------------------- | ---------------------- | ------------ | ------------------------------------------------------------------------------- |
| `RPC_URL`              | all                    | yes          | Execution-layer RPC                                                             |
| `BEACON_URL`           | synchronizer, archiver | yes          | Beacon API with blob sidecars                                                   |
| `TO_ADDRESS`           | all                    | yes          | L1 target address; must match across services (archiver reads `FILTER_ADDRESS`) |
| `INIT_START_SLOT`      | synchronizer, archiver | yes          | Beacon slot both services start from on first run                               |
| `PRIVATE_KEY`          | relayer                | relayer only | Hot wallet that signs/pays for blob txs                                         |
| `ARCHIVER_URL`         | synchronizer           | no           | Blobs older than beacon retention; unset, falls back to `BEACON_URL`            |
| `SYNC_METADATA_DB_URL` | synchronizer           | no           | Defaults to the bundled Postgres; set to point at an external one               |
| `DB_URL`               | relayer                | no           | Defaults to the bundled Postgres; set to point at an external one               |
| `BLOBS_PATH`           | archiver               | no           | On-disk blob store path; mount a volume here                                    |
| `IMAGE_TAG`            | compose                | no           | Image tag to run; pin to a release                                              |
| `APP_STATE_DB_PATH`    | synchronizer           | no           | RocksDB cache path; mount a volume here                                         |
| `HTTP_BIND`            | all                    | no           | Defaults to `0.0.0.0:3000` / `:3200` / `:3001`                                  |
| `RUST_LOG`             | all                    | no           | e.g. `info`                                                                     |

The synchronizer's `/var/lib/don` holds RocksDB - a rebuildable cache (the
authoritative state lives in Postgres), but mounting a volume avoids a slow cold
re-sync on restart. The relayer keeps no local state. The archiver's
`/var/lib/don` holds its blob store on disk (no database); mount a volume so
the archive survives restarts instead of re-downloading from the chain.

## Operational notes

- Each service is a **singleton** in a single deployment. Never run two against
  the same database or volume (the relayer collides on nonces, the synchronizer
  on slot commits, the archiver on its blob-store directory). The network can
  have many independent archivers; just never point two at one volume.
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
docker build --target archiver     -t ghcr.io/dobjlabs/digital-objects-network/archiver:dev .
# then set IMAGE_TAG=dev in deploy/.env
```
