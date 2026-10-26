# Synchronizer

Service that tracks Digital Object blob transactions on Ethereum, derives the Digital Objects Network (DON) state, persists its Merkle containers in RocksDB, stores the committed state of each slot in Postgres, and serves proof-backed query APIs over HTTP.

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
   - opens the Merkle containers of the head `DonState`
   - derives the next `DonState`
4. Publishes the new slot/state and its created-index rows in one Postgres transaction, and serves state/proof APIs for clients like `gui`.

## State model

The synchronizer splits state into two layers:

- RocksDB (`ContainerDb`) stores the content-addressed POD2 Merkle backing store
  - persistent Merkle nodes
  - persistent POD2 values
- Postgres (`StateLogDb`) stores the state log: how the DON state advances with each update to
  the bulletin board the synchronizer follows (currently blob space, one update per slot)
  - slot metadata
  - the `DonState` derived at each slot with an execution block
  - the created index: object commitment -> position in the created array

No `DonState` is stored in RocksDB, and there is no resident in-memory state cache. Every slot derivation and API read loads the head state from Postgres, then reopens the needed persistent containers from RocksDB by root.

### `DonState`

`DonState` is the compact committed DON state snapshot stored on slot rows. It is split into:

- `StateRoots`
  - `created`
  - `nullifiers`
  - `prior_state_history`
  - `next_state_history`
- `StateMetadata`
  - `state_root`
  - `block` (number, timestamp, hash)
  - `created_count`
  - `nullifier_count`
  - `state_root_count`

`StateRoots.prior_state_history` is the prior-state root array root committed inside `txlib::StateHeader`.
`StateRoots.next_state_history` is the full state root-history root after appending `StateMetadata.state_root`.

### Genesis

`INIT_START_SLOT` is the genesis slot: a finalized slot with an execution block, which the
synchronizer checks on first run (waiting for finalization if needed). It is derived like any
other slot, from empty containers, and its state root grounds transactions like any later one.
No DON transactions may exist before it. `just ensure-start-slot` picks the latest finalized
slot.

## Storage model

### RocksDB (`CONTAINER_DB_PATH`) — persistent Merkle backing store

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

Important: RocksDB is not the source of truth. It is an append-only backing store for Merkle data. Derivation may materialize nodes that are never committed; those orphaned nodes are harmless and are ignored unless a future `DonState` points at them.

### Postgres (`STATE_LOG_DB_URL`) — the state log

- `slots`
  - one row per slot from `INIT_START_SLOT` on (checked at startup); column prefixes say where
    the data comes from: `cl_` the consensus layer, `el_` the execution layer, `don_` the DON
    state
  - `cl_block_root`: `NULL` when the beacon chain produced no block
  - `el_block_*`: the execution block that became canonical with the slot, and `don_*`: the
    `DonState` derived at it; all set or all `NULL` (enforced by a `CHECK`)
- `created_index`
  - reverse index from object commitment to its position in the created array
  - one row per created object: `commitment` (primary key), `array_index`, `slot`
  - rows are inserted in the same transaction that commits their slot and
    deleted in the same transaction that rolls the slot back, so the index
    never diverges from the committed state
  - reads treat the index as a hint: every membership and proof answer
    cross-checks it against the created array at the queried root

The head state is the `DonState` of the highest slot with an execution block; the highest slot
overall is the last processed one. Until the genesis slot is committed, the state and progress
APIs answer `503`.

## Slot derivation rules

Each payload publishes every created object state, including object states consumed later in the same transaction. The created index records them all; their nullifiers determine which object states have been spent.

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

- the slot's execution block number
- created array root
- nullifiers set root
- prior state root-array root

Then it appends that new state root to the persistent state root history array and derives a new `DonState`.

Important: state root history advances for each processed slot, even if that slot accepted zero app transactions.

## Publish, recovery, and reorgs

### Publish

The synchronizer derives candidate state by mutating persistent containers in RocksDB. Once derivation succeeds, publication is a single Postgres transaction:

1. Insert the slot row, including the normalized `DonState` columns
2. Insert one `created_index` row per object commitment the slot added

Because the Merkle nodes were already materialized during derivation, there is no second write to RocksDB.

### Crash semantics

- If the process crashes before the Postgres commit, the previously committed head state remains in force and any newly written RocksDB nodes are just orphaned.
- If the process crashes after the Postgres commit, the published `DonState` is already durable and sufficient to reopen the persistent containers.

### Reorg handling

On reorg:

- the synchronizer finds the last common ancestor slot, walking back no further than
  `INIT_START_SLOT`
- deletes `slots` rows after the keep-point, and the `created_index`
  rows those slots added, in one transaction
- resumes syncing from the first divergent slot

`INIT_START_SLOT` is finalized, so a reorg never reaches it. If no common ancestor is found after
it anyway, every row is deleted and syncing restarts by re-deriving the genesis state.

Reorg rollback does not modify RocksDB. The surviving Postgres `DonState` determines which Merkle roots are current after rewind.

## API

- `GET /healthz`
  - returns `{"ok": true}`
- `GET /sync-progress`
  - returns `last_processed_slot`
- `GET /v1/state/head`
  - returns the head state, derived at the last slot with an execution block:
    - `slot` (the slot of that execution block; `/sync-progress` reports the last processed slot)
    - `state_root`
    - `block_number`
    - `block_hash`
    - `created_count`
    - `nullifier_count`
    - `state_root_count`
- `GET /v1/state/by_el/{block_id}`
  - `block_id` is a decimal execution block number or a `0x`-prefixed 32-byte execution block hash
  - returns the state derived at that execution block, with the same fields as `/v1/state/head`
  - `400` for a malformed `block_id`, `404` when no committed slot holds that block
- `POST /v1/state/membership`
  - request body:
    - `object_commitments` (array of hash strings)
    - `nullifiers` (array of hash strings)
  - returns membership for both sets from one captured state:
    - `block_number` (execution block of the head state the results are answered against)
    - `block_hash`
    - `state_root`
    - `created_results` (array of `{ commitment, present }`)
    - `nullifier_results` (array of `{ nullifier, present }`)
- `POST /v1/state/object/contains`
  - request body:
    - `object_commitments` (array of hash strings)
  - returns:
    - `block_number` (execution block of the head state the results are answered against)
    - `block_hash`
    - `state_root`
    - `results` (array of `{ commitment, present }`)
- `POST /v1/state/nullifier/contains`
  - request body:
    - `nullifiers` (array of hash strings)
  - returns:
    - `block_number` (execution block of the head state the results are answered against)
    - `block_hash`
    - `state_root`
    - `results` (array of `{ nullifier, present }`)
- `POST /v1/txlib/grounding-witness`
  - request body:
    - `objectCommitments` (array of hash strings)
  - returns:
    - `stateRoot`
    - `blockNumber`
    - `createdRoot`
    - `nullifiersRoot`
    - `priorStateHistoryRoot`
    - `createdProofs` (array of `{ commitment, present, index, proof }`)

Membership and grounding-witness requests read the head state and the
created index from one `REPEATABLE READ` Postgres transaction, so the index
entries always match the captured state's roots. The RocksDB reads then run
against those pinned roots; Merkle nodes are content-addressed and immutable
by root, so a concurrent commit or rollback cannot change what the captured
roots resolve to.

Hashes in request and response bodies are pod2 `Hash` values, serialized as lowercase 64-character hex with no `0x` prefix; inputs must use that same form (a `0x` prefix or wrong length is rejected).

## Required env vars

- `RPC_URL`
- `BEACON_URL`
- `TO_ADDRESS`
- `INIT_START_SLOT` (the genesis slot, `>= 1`: a finalized slot with an execution block, where syncing starts on first run; resumed from stored progress afterward, and must not change once the database has synced)

## Optional env vars

- `CONTAINER_DB_PATH` (default: `data/synchronizer-db`)
- `STATE_LOG_DB_URL` (default: `postgres://postgres@localhost:5432/synchronizer`)
- `HTTP_BIND` (default: `127.0.0.1:3000`)
- `RPC_RETRIES` (default: `6`) — how many times to ask again for a beacon block, execution block or archived blobs that a lagging node does not serve yet. Failed requests (rate limits, gateway errors, broken connections) are retried separately by the clients, with exponential backoff.
- `RPC_RETRY_MS` (default: `1000`) — delay in ms between those requests.
- `SYNC_DELAY_MS` (default: derived from RPC rate limit, currently `173`) — delay in ms between slots when at head. Override with caution to avoid exceeding the RPC rate limit.
- `CATCHUP_BATCH_SIZE` (default: derived from RPC rate limit, currently `7`) — number of slots fetched concurrently during catch-up. Override with caution to avoid exceeding the RPC rate limit.

The defaults for `SYNC_DELAY_MS` and `CATCHUP_BATCH_SIZE` are computed from the known RPC rate limit (15 req/s) to stay safely under the limit. If either is set via env, a warning is logged as a reminder to check rate limit compliance.

## Run

```bash
RUST_LOG=info cargo run --release --bin synchronizer
```
