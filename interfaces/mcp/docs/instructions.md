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
`help` command for the list, `list_commands` for saved ones, and the `start`
prompt for a focused command session.

## Core concepts

**Digital Objects.** Each object is a key-value dictionary (fields like
`durability`, `key`, `work`) paired with a ZK proof that the object
was created or transformed by a valid sequence of actions. The proof is
constant-size regardless of history — it does not reveal how many
transitions occurred or when the object was created.

**Classes.** Every object belongs to a class. The class is determined by
a podlang predicate — a declarative rule that defines all valid ways the
object could have reached its current state. Use `list_classes` and
`inspect_class` to discover the available classes and how they relate.

**Actions.** Actions are state transitions that consume input objects,
generate a ZK proof, and produce output objects. Each action takes
seconds to minutes of CPU time for proof generation. **Multiple actions
can run concurrently** — go ahead and call `run_action` in parallel if
the user has work to batch. Use `list_actions` to see what's available
and what each action requires.

**Nullifiers and liveness.** When an action consumes an object, it
publishes a nullifier (a hash derived from the object's key). This
prevents double-spending. An object is "live" if its nullifier has not
been published. Dead objects remain in objects for reference but
cannot be used as inputs.

**State root.** A state root is a hash of all published
transactions and nullifiers at a given Ethereum block. Actions must be
grounded in a recent state root (within ~300 blocks / ~1 hour). The
`get_state_root` tool returns the current state root.

## Tools

### Inspection

- `list_objects` — every object the user holds, with class + liveness
- `list_actions` — every available action with its required inputs
- `list_classes` — every known object class with live counts and producing/consuming actions
- `inspect_object(file_name)` — full detail on one object: fields, status, predicate source
- `inspect_class(class_name)` — predicate definition + which actions produce/consume the class
- `check_feasibility(action_id)` — does the user's objects have what this action needs?
- `get_state_root` — current state root from the synchronizer
- `read_doc(name)` — reference docs (`podlang-reference`, `object-lifecycle`,
  `how-it-works`, `command-examples`, `txlib.podlang`, `generated.podlang`, or
  `list` to enumerate)

### Mutation

- `run_action(action_id, inputObjectPaths)` — start an action; returns a
  `runId` immediately (the proof + commit run in the background). See
  "running actions" below.
- `get_run(run_id)` — poll a run's status, result/error, and progress log.

### Configuration

- `read_settings` — current synchronizer + relayer URLs the daemon is using
- `write_settings({ synchronizerApiUrl, relayerApiUrl })` — update both URLs (pass current values for fields you don't want to change)
- `get_objects_dir` — filesystem path to `~/.dobj/objects/` (useful for showing the user where their objects live)

## Recommended workflow

- Start with `list_objects` and `list_actions` to understand what's available.
- Use `check_feasibility` before `run_action` to verify inputs exist (and to confirm the action's required input classes).
- Use `inspect_object` / `inspect_class` to understand state and predicates.
- After a run reaches `succeeded`, call `list_objects` again to see the updated state.

## Running actions

`run_action` does not block. It returns immediately with a `runId` and
`status: queued`; proof generation and the commit run in the background
(seconds to minutes).

To wait for the result, poll `get_run(run_id)` until `status` is terminal:

- `succeeded` → read `result` (old/new state root, output and nullified files)
- `failed` → read `error`

The returned state also carries the ordered `progress` log, so you can show
the user which step a run is on while it's still `generateProof` or
`committing`. Multiple runs proceed concurrently — start several and poll each
`runId` independently.

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
- `GtEq`, `Equal`, `SumOf` — arithmetic constraints
- `HashOf` — hash computation

The top-level pattern is always
`IsClassName(state) = OR(Action1(...), Action2(...))`, meaning the
object's current state must be reachable via at least one valid action.
