# Digital Objects Network MCP Server

You are connected to a Digital Objects Network instance. This server lets you
inspect and manipulate Digital Objects — items whose entire existence is
proved by zero-knowledge proofs. There is no central database of
objects; each object is a self-contained ZK certificate that its holder
stores locally.

## Commands

Beyond the tools, this server offers **commands** -- named, reusable flows (some
built in, plus any the user has defined). When the user types a command's name,
or a short phrase that clearly refers to one, call `get_command(name)` to load
its full body and follow it exactly: the body governs which tools to call and
the output format, and anything typed after the name is its argument. Run the
`help` command for the list and `list_commands` for saved ones. When the client
does not expose MCP prompts directly, call `invoke_prompt({ name, arguments? })`
and follow the returned messages; use the `start` prompt for a focused command
session.

## Core concepts

**Digital Objects.** Each object is a key-value dictionary (fields like
`durability`, `key`) paired with a ZK proof that the object
was created or transformed by a valid sequence of actions. The proof is
constant-size regardless of history — it does not reveal how many
transitions occurred or when the object was created.

**Classes.** Every object belongs to a class. The class is determined by
a podlang predicate — a declarative rule that defines all valid ways the
object could have reached its current state. Use `list_classes` and
`inspect_class` to discover the available classes and how they relate.

**Actions.** Actions are state transitions that consume input objects,
generate a ZK proof, and produce output objects. Each action takes
seconds to minutes of CPU time for proof generation. Multiple runs can be
accepted at once. Each waits for an in-memory proof semaphore, reporting
`queued` until it acquires a permit. The limit is one by default, configurable
with `DOBJD_MAX_PROOFS`. A run releases its permit after recording, so submission
and waiting for the chain can overlap with the next proof. Queued runs do not
survive a daemon restart. For finer control, `start_proof_job` proves without
submitting and `submit_transactions` posts when you choose.
Use `list_actions` to see what's available and what each action requires.

**Nullifiers and liveness.** When an action consumes an object, it
publishes a nullifier (a hash derived from the object's key). This
prevents double-spending. An object is "live" if its nullifier has not
been published. Dead objects remain in objects for reference but
cannot be used as inputs. While a run or proof job is in progress, each
object it was given as an input reports that id in `heldByRunId`; a run
keeps it until its transaction lands or expires. An input of a recorded
or submitted transaction stays `live` on disk but is refused as an input
until the chain decides; `list_transactions` shows which.

**State root.** A state root is a hash of all published
transactions and nullifiers at a given Ethereum block. Actions must be
grounded in a recent state root (within ~300 blocks / ~1 hour). The
`get_state_root` tool returns the current state root.

## Tools

### Inspection

- `list_objects` — every object the user holds, with class + liveness
- `list_actions` — every available action with its required inputs
- `list_classes` — every known object class with live counts and producing/consuming actions
- `inspect_object({ file_name })` — full detail on one object: fields, status, predicate source
- `inspect_class({ class: { pluginName, name } })` — predicate definition + which actions produce/consume the class
- `inspect_action({ action: { pluginName, name } })` — predicate definition + input/output classes
- `check_feasibility({ action: { pluginName, name } })` — does the user's objects have what this action needs?
- `get_state_root` — current state root from the synchronizer
- `read_doc(name)` — reference docs (`podlang-reference`, `object-lifecycle`,
  `how-it-works`, `command-examples`, `txlib.podlang`, `generated.podlang`, or
  `list` to enumerate)

### Mutation

- `start_run({ action: { pluginName, name }, inputObjectPaths })` — start an action end to end; returns a
  `runId` immediately (prove, record, submit and follow to the chain run in the background). See
  "running actions" below.
- `get_run({ run_id })` — poll a run's status, result/error, and progress log.
- `list_runs({ active_only? })` — the runs the daemon still holds, oldest
  first; use it to find a run whose `runId` you no longer have.

### Stages

- `start_proof_job({ action, inputObjectPaths, args? })` — prove and record a
  transaction without submitting it; returns a `jobId` immediately.
- `get_proof_job({ job_id, wait_ms? })` — the job's state; `done` carries `txFinal`.
- `list_proof_jobs({ active_only? })` — the proof jobs the daemon still holds.
- `submit_transactions({ txFinals })` — post recorded transactions to the relayer.
- `list_transactions()` — every transaction record on disk: `recorded`,
  `submitted`, `landed` or `expired`. Survives a daemon restart.
- `get_transaction({ tx_final, wait_ms? })` — one record; with `wait_ms`,
  held until it lands or expires (capped by the daemon; call again while open).
- `delete_transaction({ tx_final })` — abandon a recorded transaction that
  was never offered to the relayer.

### Configuration

- `read_settings` — the daemon's current synchronizer + relayer URLs and its `mcpEnabled` toggle
- `write_settings({ synchronizerApiUrl?, relayerApiUrl?, mcpEnabled? })` — update a subset of settings; any field you omit keeps its current value. Setting `mcpEnabled` to `false` shuts down the MCP server you are connected through.
- `get_objects_dir` — filesystem path to `~/.dobj/objects/` (useful for showing the user where their objects live)

## Recommended workflow

- Start with `list_objects` and `list_actions` to understand what's available.
- Use `check_feasibility` before `start_run` to verify inputs exist (and to confirm the action's required input classes).
- Use `inspect_object` / `inspect_class` to understand state and predicates.
- After a run reaches `succeeded`, call `list_objects` again to see the updated state.

## Running actions

`start_run` does not block. It returns immediately with a `runId` and
`status: queued`; proof generation and the commit run in the background
(seconds to minutes).

To wait for the result, poll `get_run({ run_id })` until `status` is terminal:

- `succeeded` → the transaction landed; read `result` (grounding root, output files, the input files it spent)
- `failed` → read `error` (a proof or input error, or the transaction expired without landing)

The returned state also carries the ordered `progress` log, so you can show
the user which step a run is on while it's still `generateProof` or
`committing` (which covers shrinking, recording, submitting and waiting for the
chain). Poll each `runId` independently.

To separate proving from submitting, use `start_proof_job`, then `get_proof_job` with
`wait_ms` until it is `done`, then `submit_transactions` with its `txFinal`
when ready, and `get_transaction` with `wait_ms` until it is `landed` or
`expired`. Proof jobs are not queued behind the run slot, so start only as
many as the machine should prove at once.

Pass `inputObjectPaths` explicitly (one per the action's required input
classes, in order); resolve them from `list_objects` / `check_feasibility`
first. An input count that doesn't match the action makes the run fail.

## Podlang predicates

The `predicateSource` field on objects and classes shows the podlang
definition. Podlang is a declarative language for specifying ZK proof
constraints:

- `AND(...)` — all clauses must hold
- `OR(...)` — any one branch must hold (used for state-machine patterns)
- `DictContains(dict, "key", value)` — the dictionary contains this key-value pair
- `DictInsert/DictUpdate/DictDelete` — dictionary mutation operations
- `GtEq`, `Equal`, `Sum` — arithmetic constraints
- `Hash` — hash computation

The top-level pattern is always
`IsClassName(state) = OR(Action1(...), Action2(...))`, meaning the
object's current state must be reachable via at least one valid action.
