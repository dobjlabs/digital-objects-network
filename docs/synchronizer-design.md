# Synchronizer Design

## Purpose

The synchronizer is the canonical state engine for zk-craft.

It watches Ethereum beacon slots, finds blob transactions sent to the configured
`TO_ADDRESS`, verifies that those blobs contain valid `TxFinalized` proof
payloads, derives the next canonical app state, and serves read/proof APIs for
clients such as the desktop app.

At a high level it turns:

1. beacon chain data,
2. execution-layer transactions,
3. blob sidecars, and
4. zk proof payloads

into a canonical, queryable application state.

## Responsibilities

The synchronizer is responsible for:

- tracking canonical slot progress,
- rebuilding application state from chain data,
- validating proof payloads against recent canonical state,
- persisting Merkle-backed app state,
- handling reorgs by rewinding canonical metadata, and
- serving membership and grounding-witness APIs.

It is not responsible for:

- generating proofs,
- submitting transactions,
- storing users' plaintext local objects, or
- deciding which transactions should be posted to chain.

Those responsibilities live in `app-gui` and `relayer`.

## Code Map

- `synchronizer/src/main.rs`
  - boots config, RocksDB, Postgres, the HTTP API, and the sync loop
- `synchronizer/src/config.rs`
  - env loading and runtime knobs
- `synchronizer/src/sync_loop.rs`
  - startup bootstrap, catch-up loop, near-head loop, and reorg handling
- `synchronizer/src/node.rs`
  - integration layer for beacon RPC, execution RPC, blob fetching, and slot derivation
- `synchronizer/src/state_machine.rs`
  - proof validation and pure app-state derivation logic
- `synchronizer/src/app_db.rs`
  - RocksDB-backed persistent Merkle storage
- `synchronizer/src/sync_db.rs`
  - Postgres canonical metadata store
- `synchronizer/src/api.rs`
  - HTTP read and proof endpoints
- `synchronizer/src/head.rs`
  - canonical head data model

## Design Goals

The implementation is built around a few invariants:

1. Postgres is the sole source of canonical truth.
2. RocksDB is only a persistent backing store for Merkle nodes and values.
3. Canonical publication is a Postgres write, not a RocksDB write.
4. Reorg rollback deletes canonical metadata only; it does not rewrite RocksDB.
5. Read APIs anchor themselves to one Postgres snapshot before reading RocksDB.

These invariants let the synchronizer tolerate crashes and reorgs without
maintaining a complex in-memory cache of canonical state.

## State Model

The synchronizer splits state into two layers.

### 1. RocksDB: persistent Merkle backing store

RocksDB stores the content-addressed POD2 data needed to reopen application
containers by root.

Current containers:

- transactions: persistent `Set`
- nullifiers: persistent `Set`
- gsr history: persistent `Array`

The keyspaces are:

- `n/...` for Merkle nodes
- `v/...` for POD2 values

See `synchronizer/src/app_db.rs`.

RocksDB is append-only from the synchronizer's point of view. Deriving a slot
may materialize nodes that never become canonical. Those nodes are harmless
because they are ignored unless some canonical head later points at their roots.

### 2. Postgres: canonical control plane

Postgres stores one row per canonical slot in `canonical_slots`.

Each row contains:

- per-slot metadata:
  - slot number
  - beacon block root
  - parent root
  - execution block number
  - slot-level `current_gsr`
  - whether the slot was empty
- the full canonical head after that slot:
  - transactions root
  - nullifiers root
  - prior-GSR root committed inside the state root
  - full GSR-history root
  - current canonical GSR
  - current block number
  - tx count
  - nullifier count
  - GSR count

The highest committed `slot` is the current canonical head.

See `synchronizer/src/sync_db.rs`.

## Canonical Head

The current application state is represented by `CanonicalHead`:

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

See `synchronizer/src/head.rs`.

The important distinction is:

- `CanonicalHead` is compact and canonical.
- It does not embed full Merkle containers.
- Full containers are reopened from RocksDB on demand using the roots in the head.

## What A GSR Means

`txlib::StateRoot` is the compact committed state that transactions are grounded
against. It currently commits:

- execution block number,
- transactions root,
- nullifiers root, and
- prior-GSR history root.

Its hash is the canonical global state root, or GSR.

This is slightly more precise than the older high-level model of "hash the
transactions root and nullifiers root." In the current implementation the GSR
also commits the execution block number and the prior GSR-history root.

When the synchronizer derives a present slot, it computes a new `StateRoot` from
the updated transactions/nullifiers roots plus the previous GSR-history root,
hashes it, and appends that hash to the persistent GSR history array.

See `txlib/src/lib.rs` and `synchronizer/src/state_machine.rs`.

## Payload Model

The synchronizer ultimately validates `TxFinalized` proof payloads.

The binary payload format in `common/src/payload.rs` includes:

- the proof bytes,
- `tx_final`
  - the globally unique finalized transaction commitment,
- `state_root_hash`
  - the grounding GSR claimed by the proof, and
- `nullifiers[]`
  - the consumed object-state nullifiers for that transaction.

Candidate app blobs are identified in two steps:

1. execution-layer filtering:
   - only blob transactions whose `to` address matches configured `TO_ADDRESS`
     are considered
2. payload parsing:
   - `ProofParser` attempts to decode the blob bytes using zk-craft's payload
     magic and proof format
   - blobs that do not decode into a valid `TxFinalized` payload are skipped

## Startup And Bootstrap

On startup the synchronizer:

1. loads env config,
2. opens RocksDB,
3. connects to Postgres and creates schema if needed,
4. builds the proof parser,
5. creates the beacon and execution RPC clients,
6. initializes sync state, and
7. starts the HTTP API and sync loop concurrently.

Bootstrap is handled in `initialize_sync()` in `synchronizer/src/sync_loop.rs`.

The key bootstrap rule is:

- the database is initialized with a canonical row for `start_slot - 1`,
- not for `start_slot` itself.

That bootstrap row gives the synchronizer a stable "previous canonical slot" to
build from. If `INITIAL_START_SLOT` is unset, `start_slot` defaults to the
current beacon head slot, so the first row inserted is `head_slot - 1`.

The bootstrap slot row may represent:

- a real slot with block metadata, or
- an empty slot with null block metadata.

The bootstrap row's head is `CanonicalHead::empty()`.

## Sync Loop

The sync loop has two modes.

### Catch-up mode

If the synchronizer is far enough behind the beacon head, it fetches a batch of
slots concurrently using `catchup::fetch_batch()`.

Per slot it then:

- handles missing-slot semantics, or
- derives and commits a present slot.

Batch fetch is bounded by `CATCHUP_BATCH_SIZE`, which defaults to a conservative
value derived from the known RPC rate limit.

### Near-head mode

When close to the head, the synchronizer processes one slot at a time.

It uses a `HeadTracker` that:

- subscribes to beacon `head` SSE events,
- falls back to polling every 12 seconds if needed, and
- explicitly looks up a target slot to distinguish "not yet reached" from
  "skipped/empty slot".

This avoids treating missing data as final too early while still making progress
if the event stream is stale.

## Empty Slots Versus Present Slots

The synchronizer treats empty beacon slots and present beacon slots differently.

### Empty slot

If a slot has no beacon block header:

- the loop inserts a canonical `canonical_slots` row for that slot,
- marks it `is_empty = true`,
- stores no block root or block number, and
- reuses the previous `CanonicalHead` unchanged.

An empty slot does **not** create a new GSR.

### Present slot

If a slot has a beacon block:

- the synchronizer resolves its execution payload,
- derives a new head from that block's contents, and
- commits the resulting head as the new canonical state.

A present slot creates a new GSR even if it accepted zero app transactions.

## Slot Derivation Pipeline

For a present slot, the flow in `Node::derive_from_context()` is:

1. Load the current canonical base head from Postgres.
2. Build a `SlotContext` from the beacon block and execution payload.
3. Load the recent canonical GSR window from Postgres.
4. If there are no blob commitments in the beacon block, derive a new head with
   zero blob payloads.
5. Otherwise fetch the full execution block by hash.
6. Filter execution transactions down to blob transactions whose `to` address
   matches configured `TO_ADDRESS`.
7. If no matching transactions exist, derive a new head with zero blob payloads.
8. Collect all versioned blob hashes for matching transactions.
9. Fetch the corresponding blob sidecars from the beacon API.
10. Decode each blob from EIP-4844 blob bytes into the repo's compact blob
    payload bytes.
11. Pass those decoded payloads to the state machine.
12. Return a `ProcessedSlot` containing canonical metadata and the derived head.

This means the synchronizer ignores:

- blob transactions sent to other addresses, and
- non-blob transactions entirely.

It only cares about the subset of blobs intended for this app instance.

## Ordering And Determinism

This design relies on the fact that Ethereum gives a total
ordering and that validity can be computed deterministically from that order.

Canonical processing order is:

1. canonical beacon slot order,
2. execution transaction order within the slot's execution block, then
3. blob index order within each matching blob transaction.

Given that order, plus deterministic validation rules in
`StateMachine::process_blob()`, different synchronizer instances should derive
the same canonical roots and GSR history from the same canonical chain.

## Proof And Payload Validation

The state machine validates each blob independently and fail-soft:

- malformed or irrelevant blobs are skipped,
- invalid proofs are skipped,
- stale or unknown grounding roots are rejected,
- duplicate `tx_final` values are rejected,
- duplicate nullifiers are rejected,
- one bad blob does not fail the entire slot.

Validation order in `StateMachine::process_blob()`:

1. parse and verify the blob as a `TxFinalized` payload,
2. require `payload.state_root_hash` to exist in recent canonical GSR history,
3. require the grounding GSR to be within `MAX_GSR_AGE_BLOCKS`,
4. reject duplicate `tx_final`,
5. reject duplicate nullifiers, both within the payload and against current
   canonical/in-slot state.

If the payload passes those checks, the synchronizer mutates the working
transactions and nullifiers sets and updates counts.

If the payload fails those checks, it contributes nothing new to canonical
state:

- its `tx_final` is not inserted,
- none of its new nullifiers are inserted, and
- canonical counts do not advance because of that payload.

Previously spent nullifiers remain spent because they were already present in
the canonical nullifier set before this payload was evaluated.

See:

- `common/src/payload.rs`
- `common/src/proof.rs`
- `synchronizer/src/state_machine.rs`

## Object-State Semantics

The synchronizer does not store plaintext object inventories. It stores the
canonical transaction set, the canonical spent-nullifier set, and GSR history.

That means object-state validity is always time-relative.

An object state is valid relative to some GSR if:

1. there exists a canonical source transaction in the transactions set whose
   `live` set contains that object state, and
2. the object's nullifier is absent from the canonical nullifiers set at that
   same head.

This means:

- a state may have been valid at some prior GSR,
- but it may no longer be valid now if a later canonical transaction nullified
  it.

The synchronizer's APIs support this model by exposing:

- transaction membership checks,
- nullifier membership checks, and
- source-transaction Merkle proofs for grounding.

## Working State During Derivation

Deriving one present slot opens a temporary `WorkingState`:

- `transactions` set from the base head
- `nullifiers` set from the base head
- `gsr_history` array from the base head
- `recent_gsrs` lookup map
- mutable metadata counters

This working state is ephemeral in memory, but its underlying persistent sets
and arrays may materialize new nodes in RocksDB as they are mutated.

After all blobs are processed, the state machine:

1. computes the next GSR from the updated roots,
2. appends it to the GSR history array,
3. packages the new roots and metadata into a `CanonicalHead`, and
4. returns that head to the caller.

At this point the new head is only a candidate canonical state. It becomes
canonical only when Postgres commits a new `canonical_slots` row.

## Canonical Publish

Canonical publication is intentionally small:

1. derive state and materialize any needed Merkle nodes in RocksDB,
2. create a `CommittedSlotRecord`,
3. insert one Postgres row into `canonical_slots`.

There is no second canonical write to RocksDB.

This gives simple crash semantics:

- crash before Postgres commit:
  - old canonical head remains active,
  - newly written RocksDB nodes are orphaned but harmless
- crash after Postgres commit:
  - the canonical head is already durable,
  - RocksDB can reopen the canonical containers from the committed roots

## Reorg Handling

Reorg logic lives in `synchronizer/src/sync_loop.rs`.

The loop rewinds when it detects any of these conditions:

- a previously present slot is now missing,
- a previously empty slot is now present,
- the stored block root for a slot differs from the live block root,
- a present block's `parent_root` does not match the previously stored slot.

When that happens the synchronizer:

1. walks backward to find the last common ancestor slot,
2. deletes `canonical_slots` rows after that keep-point, and
3. resumes syncing from the first divergent slot.

Rollback does not touch RocksDB. The surviving Postgres head determines which
roots are canonical after rewind.

## Read Path And API Semantics

The API is designed so responses are anchored to one canonical snapshot.

The read path is:

1. load one `CurrentSnapshot` from Postgres,
2. extract the canonical roots and metadata from that snapshot,
3. reopen RocksDB containers or generate proofs against those exact roots, and
4. return results tagged with the same head information.

This prevents a single response from mixing different canonical heads across
multiple reads.

Current endpoints:

- `GET /healthz`
- `GET /sync-progress`
- `GET /v1/state/head`
- `POST /v1/state/membership`
- `POST /v1/state/tx/contains`
- `POST /v1/state/nullifier/contains`
- `GET /v1/state/tx/{tx_hash}`
- `POST /v1/txlib/grounding-witness`

Important API behaviors:

- membership endpoints accept up to 256 total queried hashes,
- hash parsing accepts `0x`prefixed and raw hex,
- grounding-witness responses include Merkle proofs for requested source
  transactions against the canonical transactions root.

## Why The Grounding-Witness API Exists

The desktop app needs proof-bearing evidence that its input source transactions
are included in the current canonical transactions set.

The synchronizer answers that with `POST /v1/txlib/grounding-witness`, which
returns:

- the current canonical state root fields,
- the canonical state root hash, and
- per-source transaction Merkle proofs.

That response is what lets `app-gui` and `craft_sdk` build new transactions
grounded in the current canonical state.

## Operational Knobs

Important env vars:

- `RPC_URL`
- `BEACON_URL`
- `TO_ADDRESS`
- `APP_STATE_DB_PATH`
- `SYNC_METADATA_DB_URL`
- `HTTP_BIND`
- `INITIAL_START_SLOT`
- `SYNC_DELAY_MS`
- `CATCHUP_BATCH_SIZE`
- `RPC_RETRIES`
- `RPC_RETRY_MS`

The defaults for `SYNC_DELAY_MS` and `CATCHUP_BATCH_SIZE` are derived from the
assumed RPC rate limit to avoid overrunning providers during normal operation.

## Mental Model

The simplest accurate mental model is:

- Postgres says which roots are canonical.
- RocksDB stores whatever Merkle material is needed to reopen those roots.
- Each present slot derives a new candidate head from chain data.
- Each empty slot preserves the previous head.
- Reorgs only rewrite Postgres history.
- Read APIs always anchor to one canonical Postgres snapshot before touching
  RocksDB.

That split is the main reason the synchronizer stays simple even though it needs
to survive crashes, retries, invalid blobs, and reorgs.
