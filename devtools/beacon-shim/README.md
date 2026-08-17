# beacon-shim

Beacon REST API shim over a local [anvil](https://getfoundry.sh) devnet. Development tool only: it is not built into any release image and is not part of the deployed stack.

`just dev-local` runs it alongside anvil so the archiver and synchronizer can follow a local chain instead of a public Ethereum endpoint.

## What it does

anvil simulates the execution layer, which has no notion of slots, beacon roots, or beacon blocks. This shim projects anvil's chain onto the consensus-layer view that `eth-clients` expects, one to one:

| Beacon concept | anvil equivalent |
| -------------- | ---------------- |
| slot           | block number     |
| beacon root    | block hash       |
| `parent_root`  | parent block hash |

Because anvil restores stored blocks rather than recomputing them, roots stay stable across an `anvil --state` restart, so a synchronizer that has already committed roots can resume against them.

## Endpoints

| Route | Notes |
| ----- | ----- |
| `GET /healthz` | Liveness, for `just wait-health`. |
| `GET /eth/v1/config/spec` | Only `DEPOSIT_NETWORK_ID`, read from anvil's chain id. Callers log it and never validate it. |
| `GET /eth/v1/beacon/headers/{block_id}` | `head`, `finalized`, or a slot number. 404 means the slot holds no block, which callers skip. |
| `GET /eth/v2/beacon/blocks/{block_id}` | Includes `blob_kzg_commitments`, recomputed from the blob bytes. |
| `GET /eth/v1/beacon/blobs/{block_id}` | Forwarded to anvil with the query rewritten. |
| `GET /eth/v1/events?topics=head` | One `head` event per new block. |

Two of these are less obvious than the rest.

**Commitments are recomputed.** anvil serves blobs but not their KZG commitments, and the archiver and synchronizer derive versioned hashes from the commitments and then index blobs by the resulting position. A placeholder would not merely mismatch, it would panic on the lookup, so the shim recomputes real commitments from the blob bytes with the same mainnet trusted setup the relayer uses.

**The blob query is rewritten.** anvil parses `versioned_hashes` as one comma-separated value, while `eth-clients` sends repeated query pairs. A repeated-pair request reaches anvil as the last pair alone and silently returns the wrong blob set rather than an error, so callers must go through the shim rather than talking to anvil's beacon port directly.

## Configuration

| Variable | Default | Meaning |
| -------- | ------- | ------- |
| `BEACON_SHIM_BIND` | `127.0.0.1:8555` | Address to serve on. Deliberately not `HTTP_BIND`, which every shipped service also reads, so a shared launcher environment cannot set both at once. |
| `ANVIL_RPC_URL` | `http://127.0.0.1:8545` | anvil's JSON-RPC endpoint, which also serves its beacon paths. |

It waits for anvil at startup, so the two can be launched in any order.

## Running the devnet

`just dev-local` handles all of this, but the constraints it encodes are worth knowing:

- **Block time is 2s.** Grounding state roots expire after `MAX_STATE_ROOT_AGE_BLOCKS` (300), so faster blocks can expire a root while a proof is still being generated. That surfaces as a grounding failure rather than a config error.
- **`INIT_START_SLOT` must be >= 1**, because the synchronizer bootstraps from `INIT_START_SLOT - 1`.
- **anvil state persists** to `data/anvil-state.json`, blob sidecars included, so a restart resumes the chain the archiver and synchronizer already indexed. It has to stay in step with them: `just reset` drops all of it together, and keeping one side while wiping the other leaves the synchronizer deriving against roots that no longer exist.
- **Devnet state is fully separate** from `just dev`'s, because the two point at different chains and an object created against one can be neither proven nor synced against the other. That covers the Postgres databases, the RocksDB path, the blobs directory, and — via `DOBJ_HOME` — the driver's own root, so objects, installed plugins, and `settings.json` do not mix either. Without the last of those the isolation would be pointless: the services would stay coherent while `~/.dobj` filled with objects that only work in one mode.

## Scope

A straight projection of whatever anvil reports. It holds no slot state of its own, so it cannot synthesize skipped slots or reorgs that anvil did not produce.
