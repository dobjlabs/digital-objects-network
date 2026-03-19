# ZK-Craft MCP Server

You are connected to a ZK-Craft game instance. This server lets you inspect and manipulate Digital Objects — items whose entire existence is proved by zero-knowledge proofs. There is no central database of objects; each object is a self-contained ZK certificate that its holder stores locally.

## Core concepts

**Digital Objects.** Each object is a key-value dictionary (fields like `blueprint`, `durability`, `key`) paired with a ZK proof that the object was created or transformed by a valid sequence of actions. The proof is constant-size regardless of history — it does not reveal how many transitions occurred or when the object was created.

**Classes.** Every object belongs to a class. The class is determined by a podlang predicate — a declarative rule that defines all valid ways the object could have reached its current state. Use `list_actions` and `inspect_class` to discover the available classes and how they relate.

**Actions.** Actions are state transitions that consume input objects, generate a ZK proof, and produce output objects. Each action takes seconds to minutes of CPU time for proof generation. Only one action can run at a time. Use `list_actions` to see what's available and what each action requires.

**Nullifiers and liveness.** When an action consumes an object, it publishes a nullifier (a hash derived from the object's key). This prevents double-spending. An object is "live" if its nullifier has not been published. Dead objects remain in inventory for reference but cannot be used as inputs.

**State root.** A Global State Root (GSR) is a hash of all published transactions and nullifiers at a given Ethereum block. Actions must be grounded in a recent GSR (within ~300 blocks / ~1 hour). The `get_state_root` tool returns the current GSR.

## Using the tools

- Start with `list_inventory` and `list_actions` to understand what's available.
- Use `check_feasibility` before `run_action` to verify inputs exist.
- Use `inspect_object` to see an object's fields and the predicate that certifies it.
- Use `inspect_class` to understand a class without needing a specific instance.
- `run_action` blocks for proof generation. It returns an error if another action is already running — do not retry immediately.
- After `run_action`, call `list_inventory` again to see the updated state.

## Podlang predicates

The `predicateSource` field on objects and classes shows the podlang definition. Podlang is a declarative language for specifying ZK proof constraints:

- `AND(...)` — all clauses must hold
- `OR(...)` — any one branch must hold (used for state-machine patterns)
- `DictContains(dict, "key", value)` — the dictionary contains this key-value pair
- `DictInsert/DictUpdate/DictDelete` — dictionary mutation operations
- `GtEq`, `Equal`, `SumOf` — arithmetic constraints
- `HashOf` — hash computation

The top-level pattern is always `IsClassName(state) = OR(Action1(...), Action2(...))`, meaning the object's current state must be reachable via at least one valid action.
