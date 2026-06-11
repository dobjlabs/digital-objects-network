# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project overview

This repository is the reference implementation of the Digital Objects Network: a decentralized network for creating, executing, and exchanging Digital Objects -- fully programmable state machines owned and operated by Internet users, that can be passed between mutually untrusting users (for example over email) while keeping their integrity and consistency, without relying on any central trusted authority. Objects are privately held files on disk; their state transitions are proved with POD2/Plonky2 and anchored to Ethereum blob data availability, so the chain sees only opaque commitments.

The repo ships a headless daemon (`dobjd`) that owns all driver state, several clients that drive it over HTTP/SSE/MCP (a React GUI servable in a browser or wrapped in a Tauri shell, the `dobj` CLI, an MCP server for AI agents), and the chain-side services that anchor and sync objects. The bundled `craft-basics` plugin (a small crafting game) is the demonstrated end-to-end flow.

This file focuses on navigating the code, building/testing, and gotchas.

## Workspace layout

The workspace is declared in `Cargo.toml`. Crate-by-crate:

| Crate                            | Role                                                                                                                                                                                                 |
| -------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `dobjd`                          | **The daemon.** Headless HTTP server on `:7717` wrapping the driver. Every client talks to it.                                                                                                       |
| `cli`                            | `dobj` CLI binary. Thin HTTP/SSE client of dobjd. No `Driver` of its own.                                                                                                                            |
| `interfaces/gui/src-tauri`       | Tauri 2 shell. Holds **no** driver state — webview talks to dobjd over HTTP. Native conveniences only.                                                                                               |
| `interfaces/gui/src` (TS)        | React/Vite frontend. Component-based: `features/{actions,objects,context,proof-runner,settings}`.                                                                                                    |
| `driver`                         | Headless Rust orchestration library. **The core.** Owns `~/.dobj/`, runs actions end-to-end.                                                                                                         |
| `sdk`                            | Rhai engine + two-phase Loader/Executor that compiles plugin scripts into pod2 modules.                                                                                                              |
| `txlib`                          | Transaction state machine: `StateHeader`, `GroundingWitness`, `Tx`, `TxBuilder` + `TxFinalized` rule.                                                                                                |
| `synchronizer`                   | Long-running service: ingests Ethereum blobs, maintains Merkle state, serves HTTP queries.                                                                                                           |
| `relayer`                        | HTTP service that wraps proofs as EIP-4844 blob txs and submits them.                                                                                                                                |
| `archiver`                       | Service that follows beacon blocks and archives blobs filtered by destination address to the filesystem (no DB). Serves them via a beacon-compatible HTTP API; the synchronizer reads blobs from it. |
| `eth-clients`                    | Partial Ethereum Beacon client API (adapted from Blobscan, MIT). Used by `archiver`/`synchronizer` to follow the chain.                                                                              |
| `payload`                        | Cross-crate types: blob payload encoding, plonky2 proof shrink wrapper, `BlobParser`.                                                                                                                |
| `wire-types`                     | Pure-data types crossing process boundaries (HTTP/MCP/SSE/CLI). Dependency-light — no pod2/plonky2.                                                                                                  |
| `pod2utils`                      | Macros (`st_custom!`, `dict!`, `set!`, `op!`, …) and `BuildContext` for loading podlang modules.                                                                                                     |
| `pexe`                           | `.pexe` plugin archive format (zip of `manifest.toml` + `plugin.rhai`) and the `pexe` CLI.                                                                                                           |
| `mcp` (crate name `dobj-mcp`)    | MCP server exposing driver as tools to AI agents. Embedded by dobjd on the adjacent port.                                                                                                            |
| `libs/intro-pods/vdfpod`         | VDF intro pod (PoW gating via iterated hashing).                                                                                                                                                     |
| `libs/intro-pods/lt-eq-u256-pod` | 256-bit `<=` intro pod (PoW difficulty checks). Crate name `lt-eq-u256-pod`.                                                                                                                         |
| `examples/*`                     | Example plugin sources: `craft-basics` (Log, Wood, Stick, Stone, WoodPick, StonePick + 9 actions) and `craft-rocket`.                                                                                |

## Build / test / dev

Use `just` (recipes in `justfile`):

| Recipe                   | What it does                                                                                                                                                                                                                                                                                  |
| ------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `just dev`               | Brings up archiver + synchronizer + relayer + **dobjd** + Vite + Tauri shell via `mprocs.yaml`, each gated on the previous one's health. Depends on `ensure-db` + `ensure-start-slot` + `ensure-plugins` + `ensure-mcp`. Open `http://localhost:1420` in a browser or use the desktop window. |
| `just dev-remote`        | Like `just dev` but skips the local archiver/synchronizer/relayer and points dobjd at the hosted public endpoints (via `ensure-remote-settings`). Uses `mprocs.remote.yaml`. No local Postgres needed.                                                                                        |
| `just sync`              | Runs the synchronizer (loads `services/synchronizer/.env`).                                                                                                                                                                                                                                   |
| `just relayer`           | Runs the relayer (loads `services/relayer/.env`).                                                                                                                                                                                                                                             |
| `just archiver`          | Runs the archiver (loads `services/archiver/.env`).                                                                                                                                                                                                                                           |
| `just dobjd`             | Runs the headless HTTP daemon. Default port `7717` (override via `DOBJD_PORT`).                                                                                                                                                                                                               |
| `just desktop`           | Standalone Tauri window — Tauri spawns its own Vite on `:1420`.                                                                                                                                                                                                                               |
| `just desktop-shell`     | Tauri shell pointing at an already-running Vite (used inside `just dev`). Skips `beforeDevCommand`.                                                                                                                                                                                           |
| `just web`               | Vite dev server alone on `:1420`. Talks to `dobjd` at `:7717`.                                                                                                                                                                                                                                |
| `just ensure-db`         | Creates the local `synchronizer` + `relayer` Postgres DBs if absent. Idempotent. Run before `just sync` / `just relayer` standalone.                                                                                                                                                          |
| `just ensure-start-slot` | Rewrites a fresh synchronizer/archiver `.env` `INIT_START_SLOT` to the current beacon head (no-op once the store exists and the var is set).                                                                                                                                                  |
| `just ensure-plugins`    | Installs `craft-basics.pexe` to `~/.dobj/actions/` if none present.                                                                                                                                                                                                                           |
| `just ensure-mcp`        | Registers/refreshes the dobj MCP at `http://127.0.0.1:7718/mcp` with Claude Code, project scope. Idempotent; no-op if the `claude` CLI is missing.                                                                                                                                            |
| `just install-plugins`   | Builds + installs all `examples/*` via the `pexe` CLI.                                                                                                                                                                                                                                        |
| `just pack-plugins`      | Builds plugins to `target/pexe/*.pexe` (no install).                                                                                                                                                                                                                                          |
| `just pexe *ARGS`        | Runs the `pexe` CLI with arbitrary args (e.g. `just pexe inspect plan --action CraftWood examples/craft-basics`).                                                                                                                                                                             |
| `just cli *ARGS`         | Runs the `dobj` CLI with arbitrary args (e.g. `just cli inspect-action craft-basics::FindLog`).                                                                                                                                                                                               |
| `just reset`             | Stops the dobj daemon, wipes `data/` + `~/.dobj/`, drops the `synchronizer` + `relayer` DBs, removes the archiver blobs dir, and removes the dobj MCP registration.                                                                                                                           |
| `just test`              | `cargo test --workspace --release`.                                                                                                                                                                                                                                                           |
| `just test-ignored`      | Runs `--ignored` tests with `--nocapture`.                                                                                                                                                                                                                                                    |
| `just test-e2e`          | Runs `synchronizer::test_e2e_real_proof` (slow, full real-proof flow).                                                                                                                                                                                                                        |
| `just build`             | `cargo build --workspace`.                                                                                                                                                                                                                                                                    |

**Infrastructure required for `just dev`:**

- Postgres on `localhost:5432` (user `postgres`). `just ensure-db` creates the `synchronizer` + `relayer` DBs.
- An Ethereum beacon + execution endpoint (configure in `services/synchronizer/.env`, `services/relayer/.env`, `services/archiver/.env`).
- RocksDB stores under `data/` and `~/.dobj/` are created on first run; the archiver writes blobs to its `BLOBS_PATH` (filesystem, no DB).
- `dobjd` binds **two adjacent ports**: HTTP on `DOBJD_PORT` (default `7717`) and MCP on `DOBJD_PORT + 1` (default `7718`). Both must be free or startup fails fast.
- `just dev-remote` skips the local chain-side services and points dobjd at the hosted synchronizer + relayer, so no local Postgres / beacon is needed.

**Always run tests with `--release`** — proof generation is impractically slow in debug. Use `MockProver` (not the real Prover) in unit tests; gate real-proof tests with `#[ignore]`. Use `assert!`, not `debug_assert!`, since tests run `--release`.

**Before committing:** run `cargo fmt` and `cargo clippy --tests --examples`. If you use `cargo check`, pass `--tests --examples` too.

## Architecture in 30 seconds

```
   Tauri desktop       Browser tab        dobj CLI         AI agents
   (webview, no        (Vite :1420)       (HTTP/SSE)       (MCP client)
    Driver of its own)
            \              |                  |                 /
             \             |                  |                /
              \            ▼                  ▼               /
               └───────►  dobjd HTTP :7717  ◄─────────────────┘
                           │       │
                           │       └─► MCP :7718 (DOBJD_PORT + 1)
                           ▼
                         driver  ────────► synchronizer (HTTP: grounding witness, membership)
                           │
                           ▼
                         relayer  ────────► Ethereum L1 (EIP-4844 blob)
                                                │
                                                ▼
                                           archiver (follows beacon blocks, stores blobs)
                                                │
                                                ▼
                                           synchronizer (reads blobs, builds state roots)
```

`dobjd` is the **single owner of `Arc<Driver>`** in the running system. Desktop, browser, MCP, and CLI all talk to it over HTTP/SSE; the Tauri shell holds no driver state of its own.

`Driver::execute` is the central call: validate inputs -> fetch grounding witness -> re-parse plugin script through SDK -> drive `txlib::TxBuilder` -> produce `(tx_pod, obj_pods)` -> shrink + post via relayer -> poll for confirmation -> reconcile via `sync_objects`.

## Key entry points

- **`libs/driver/src/driver.rs`** — `Driver` struct. Methods: `open_default()`, `list_objects()`, `read_object()`, `execute()`, `check_action()`, `sync_objects()`, `get_state_root()`. Public types live in `libs/driver/src/types.rs`.
- **`libs/driver/src/execute.rs`** — the proof-and-commit pipeline.
- **`libs/driver/src/pexe_catalog.rs`** — concrete `ActionCatalog` impl that loads `.pexe` archives from `~/.dobj/actions/`.
- **`libs/sdk/src/lib.rs`** — Rhai engine setup, custom syntax for `var` and `unsafe { ... }` (search `register_custom_syntax`), host API registration (search `register_fn`). Entry: `Sdk::load_module_from_src_actions`.
- **`libs/txlib/src/lib.rs`** — `StateHeader` (typed record), `GroundingWitness`, `Tx`, `TxBuilder`. No `Object` struct: object state is a `pod2::Dictionary` whose `identity` field is the commitment of its initial form (`with_identity`), stable across mutations.
- **`libs/txlib/src/predicates/txlib.podlang`** — the transaction state machine (replay, grounding, `TxFinalized`). Imports **`tx_events.podlang`**, the frozen chain-primitive batch (`TxInsert`/`TxMutate`/`TxDelete`) whose id is pinned by a test. See "txlib and the SDK" below.
- **`services/synchronizer/src/state_machine.rs`** — `MAX_STATE_ROOT_AGE_BLOCKS = 300`. Pure derivation: takes a base head + recent state roots + decoded blob bytes; returns a candidate head.
- **`services/synchronizer/src/api.rs`** — Axum routes (`/v1/state/head`, `/v1/state/membership`, `/v1/state/object/contains`, `/v1/state/nullifier/contains`, `/v1/txlib/grounding-witness`, plus `/healthz`, `/sync-progress`).
- **`libs/payload/src/shrink.rs`** — Plonky2 wrapper circuit that re-proves a MainPod in a smaller circuit so it fits in a blob.
- **`libs/payload/src/payload.rs`** — blob payload encoding (`PAYLOAD_MAGIC`, proof type, `tx_final`, state root, nullifiers, and the `live` object commitments).
- **`interfaces/mcp/src/lib.rs`** — `DEFAULT_PORT = 7718`; crate is named `dobj-mcp` (depend on it as `dobj-mcp = { path = "../mcp" }`). dobjd runs MCP at `DOBJD_PORT + 1`.
- **`services/dobjd/src/main.rs`** — daemon entry point. Binds HTTP + MCP listeners up-front (fail-fast if either port is taken), constructs `Arc<Driver>` once via `Driver::open_default()`, shares it with the embedded `dobj-mcp` server. `DEFAULT_HTTP_PORT = 7717`, override via `DOBJD_PORT`.
- **`services/dobjd/src/routes/mod.rs`** — axum routes: `/healthz`, `/objects`, `/state-root`, `/objects/{dir,file_name}`, `/classes[/{name}]`, `/settings`, `/actions[/run,/{id}[/feasibility]]`, `/actions/runs/{id}[/events]` (run-status poll + per-run replayable SSE), `/events` (SSE). dobjd is API-only; the UI is served separately.
- **`services/dobjd/src/runs.rs`** — the run registry. `POST /actions/run` is non-blocking: it registers a run, spawns a background worker, and returns a `runId`. The worker records status + progress + terminal result/error into an in-memory, TTL-reaped registry that backs `/actions/runs/{id}` (poll) and its SSE. Shared by the HTTP routes and the MCP server.
- **`services/dobjd/src/events.rs`** — broadcast hub behind SSE `/events`, shared with the MCP server so progress updates fan out to every client.
- **`interfaces/cli/src/main.rs`** — `dobj` CLI. Thin reqwest/SSE client of dobjd; no driver state.
- **`libs/wire-types/src/lib.rs`** — shared HTTP/MCP/SSE/CLI payload types (`QualifiedName`, etc.). Optional `schemars` feature for JSON Schema generation.
- **`interfaces/gui/src-tauri/src/lib.rs`** — Tauri shell, no driver state. Sub-modules: `cpu`, `error`, `objects`, `settings`. Commands cover only desktop-native conveniences; every state-touching call goes through dobjd over HTTP.

## txlib and the SDK

txlib's proof machinery is consumed only through the SDK. Its plain types (`StateHeader`, `GroundingWitness`) are shared more widely (the synchronizer computes state roots), but the predicate batches and `TxBuilder` are not: the SDK loads both batches (`txlib::predicates::events_module()` and `module()`) alongside the plugin module, drives `TxBuilder`, and is the only code that names txlib predicates.

The predicates are split across two batches. `tx_events.podlang` holds the three chain primitives -- `TxInsert`, `TxMutate`, `TxDelete` -- the only txlib predicates an action script's rendered podlang ever references (via `tx::`, rendered in `libs/sdk/src/fmt_podlang.rs`). Plugin module hashes bake in only this batch's id, so it is frozen: its id is pinned by `test_events_module_hash_pinned`, and editing it invalidates every plugin manifest and recorded proof. `TxMutate`'s shared `type` arg pins `old.type == new.type` implicitly and preserves the object's `identity` across the mutation.

`txlib.podlang` (the batch from `module()`, which imports the events batch) is fixed scaffolding that the SDK and `TxBuilder` compose automatically; you never write or reference it from a script, and it can churn without touching plugin hashes. It is the replay walker (`ReplayActions` and friends) that re-walks the recorded event chain to update the live/nullifier sets, grounding (`InputsGrounded`) that proves each input existed in a prior state root, and the top-level `TxFinalized` rule the whole transaction is proved against. The file reads bottom-up: replay -> grounding -> `TxFinalized`.

## SDK essentials

Action scripts are Rhai files; the SDK registers a host API and runs each action **twice per `execute`**:

1. **Load phase** — symbolic, no real inputs. Records the rule shape (events, statements, wildcards) and produces the compiled `CustomPredicateBatch` whose id is the manifest's `module_hash`.
2. **Execute phase** — same script with real inputs. Drives `txlib::TxBuilder`, runs intro pods (VDF, `lt_eq_u256`), produces the MainPod.

The driver does not cache compiled modules between calls — every `Driver::execute` re-parses the script.

**Rhai custom syntax** (search `register_custom_syntax` in `libs/sdk/src/lib.rs`):

- `var <ident> = <expr>` — declare a wildcard. Operations on it generate constraining statements.
- `let <ident> = <expr>` — plain Rhai, literal known at both phases. No statements emitted.
- `unsafe { <expr> }` — compute a wildcard value without emitting constraints. Pair with an explicit `action.st_*` call afterward, or a malicious prover can put anything there.

**Host API** (registered via `register_fn` in `libs/sdk/src/lib.rs`):

- On `action`: `input(class)`, `output(class)`, `mutate(class)`, `subaction(name)`, `random()`, `st_gt(a,b)`, `st_sum_of(a,b,c)`, `intro_vdf(iters, obj)`, `intro_lt_eq_u256(obj, target)`, `pow_obj_grind(obj, target)`, `top_limb_u256(n)`.
- On object handles: `set([[k,v],...])` (initializer for literals), `update(k,v)` (writes a witness-derived value), `get(k)`, indexer `obj.<field>`.

**Constraint:** the event tree must be the same shape every run. Branching that emits _different events_ on different inputs is unsupported. Branching on wildcard values inside `unsafe { ... }` is fine.

**Records-form rendering** (`libs/sdk/src/fmt_podlang.rs`): to keep predicate arity small, the formatter coalesces all same-role objects of an action into one record arg instead of N loose wildcards. The public records are `in <Action>In` / `out <Action>Out` (one entry per input/output object; a Mutate contributes to both); the private `<Action>Initials` record holds one entry per output object's pre-identity (script-final) dict. The related `chain_steps`/`<Action>Chain` packing does the same for intermediate chain states, but only past a threshold (`CHAIN_PACK_MIN_TS = 3`); the in/out/initials records have no threshold and are the canonical form.

**`Side` vs `Collapse`** (same file): these are two distinct axes, do not conflate them. `Side { In, Out }` is the strictly-binary public I/O role — it drives `dispatch_side` and the public signature. `Collapse { Side(Side), Initials }` is the record namespace a _collapsed_ object dict pins to when rendered (`in.<name>` / `out.<name>` / `initials.<name>`) or anchored; it is what `collapsed_at` / `collapses_at` return. `Initials` lives only on `Collapse` (private-only, never a public record arg or a dispatch target) and is deliberately **not** a `Side`.

## Coding style

ASCII only in code and comments: no em-dashes or other non-ASCII characters.

### Naming

Name things for what they mean, not for abstract or mathematical placeholders -- at every level:

- Variables and fields: descriptive over terse, even where it reads long, and even where an algorithm's paper uses a single letter. This is industrial code, not a transliteration of the math (`num_statements`, not `n`; `used_in_link`, not `u`).
- Types: when the same tuple of scalars keeps travelling together in one role -- across a body, its callers, a loop, or a parameter list -- make it a struct. The type name says what the value is; the field names say what each component means.

### Comments

While coding, write a comment only if all three hold:

- its subject is present in this unit -- the signature, a parameter, a local, a value's range here, a branch's precondition;
- it does not transcribe another unit's mechanism, name a specific that will rot (a fixture, a sibling function, a field elsewhere), or lean on what you'd have to leave the repo to check (a prior version, a use case, our conversation);
- it carries something an intelligent reader could not easily infer from this unit -- not control flow, not types, not return values, not general knowledge, not a paraphrase of the line beside it.

Test: cut any clause that transcribes mechanism, states general knowledge, or names a rot-prone specific; then cut any "because X" you can't check against the repo. If nothing survives, delete the comment. If what survives is inferable from the code, delete that too. Comments should not quote code directly, as this makes drift between code and comment more likely.
