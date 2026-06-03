# bitcraft MCP Server

You are connected to a bitcraft game instance. This server lets you
inspect and manipulate Digital Objects — items whose entire existence is
proved by zero-knowledge proofs. There is no central database of
objects; each object is a self-contained ZK certificate that its holder
stores locally.

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
been published. Dead objects remain in inventory for reference but
cannot be used as inputs.

**State root.** A Global State Root (GSR) is a hash of all published
transactions and nullifiers at a given Ethereum block. Actions must be
grounded in a recent GSR (within ~300 blocks / ~1 hour). The
`get_state_root` tool returns the current GSR.

## Tools

### Inspection

- `list_inventory` — every object the user holds, with class + liveness
- `list_actions` — every available crafting action with its required inputs
- `list_classes` — every known object class with live counts and producing/consuming actions
- `inspect_object(file_name)` — full detail on one object: fields, status, predicate source
- `inspect_class(class_name)` — predicate definition + which actions produce/consume the class
- `check_feasibility(action_id)` — does the user's inventory have what this action needs?
- `get_state_root` — current canonical GSR from the synchronizer
- `read_doc(name)` — reference docs (`podlang-reference`, `object-lifecycle`, `txlib.podlang`,
  `time.podlang`, `generated.podlang`, or `list` to enumerate)

### Mutation

- `run_action(action_id, input_object_paths)` — execute an action,
  blocks for proof generation. See "running actions" below.

### Configuration

- `read_settings` — current synchronizer + relayer URLs the daemon is using
- `write_settings({ synchronizerApiUrl, relayerApiUrl })` — update both URLs (pass current values for fields you don't want to change)
- `get_objects_dir` — filesystem path to `~/.dobj/objects/` (useful for showing the user where their objects live)

## Recommended workflow

- Start with `list_inventory` and `list_actions` to understand what's available.
- Use `check_feasibility` before `run_action` to verify inputs exist (and to confirm the action's required input classes).
- Use `inspect_object` / `inspect_class` to understand state and predicates.
- After `run_action`, call `list_inventory` again to see the updated state.

## Running actions

`run_action` blocks for the duration of proof generation (seconds to
minutes). Two behaviors worth knowing about:

- **Progress notifications.** If you supply a `progressToken` in the
  call's `_meta`, the server streams `notifications/progress` for each
  proof-generation and commit step. The user-visible host UI may render
  these as a status indicator. Useful for long-running actions where you
  want the user to see something is happening.
- **Elicitation for ambiguous inputs.** If you call `run_action` with an
  empty `input_object_paths` (or omit the field), the server resolves
  bindings from the user's inventory:
  - 0 candidates for a required class → returns an error
  - 1 candidate → bound automatically (no prompt)
  - 2+ candidates → server sends an `elicitation/create` request with a
    form asking the user to pick one per ambiguous class. The user's
    answer is used as the input bindings.

  If the user has clearly indicated which object to use, you can pass
  `input_object_paths` explicitly and skip the elicitation round-trip.
  When in doubt, leave the array empty and let the user choose.

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
