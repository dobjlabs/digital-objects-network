# txlib

Transaction predicates for verifiable state transitions in the Digital Objects Network.

## SDK: actions and classes

The SDK defines **actions** and **classes**. A class is a label for an object's type: what kind of thing it is. Some actions output objects of class "Wood"; others take "Wood" as input. The class is defined by the set of actions capable of producing, mutating, or deleting an object of that class. Concretely, every object dictionary carries a `type` field whose value is the hash of an `Is<Class>` predicate. That predicate is an OR over all the actions in the class.

## Transactions: verifying state changes

A transaction verifies a set of state changes: newly created object states and nullifications of existing object states. It checks both that the initial states are valid (grounded in the prior global state) and that every change (creating a new object, mutating or deleting an existing one) has a valid cause.

For this to work, actions need a way of saying "these are the changes I caused", and the system needs a way to look at any change and ask whether there is a valid cause for it: did this change originate in one of the permitted actions for the affected object's class?

## Action statements

Before the system can answer that question, the prover first produces **action statements**: proofs of action predicates like `FindLog`, `CraftWood`, `UseWoodPick`. Each action statement commits to:

- The set of inputs the action consumed (possibly empty; `FindLog` takes none).
- The set of outputs the action produced (possibly empty; a pure-delete action produces none).
- A range on the transaction's hash chain (see next).

## The hash chain

Every event in a transaction (insert, mutate, delete) is recorded as one step of a chained hash. For each event a small per-event hash is computed (insert: `H({}, new)`; mutate: `H(old, new)`; delete: `H(old, {})`, where `{}` is the empty value) and folded into the running chain (`chain = H(prev_chain, event_hash)`). The chain is the canonical, ordering-preserving commitment to "what happened, in what order".

An **action's range** is the pair `(chain_start, chain_end)` that brackets the action's events. The first event in the action takes `chain_start` as its `prev_chain`; the last event produces `chain_end`. The action statement commits to its range publicly so other proofs can match against it.

## Sub-actions and recursive ranges

An action can contain sub-actions: action statements whose ranges sit inside the parent action's range. `MineStone`'s body has a `UseWoodPick` sub-action (mutate the pick) followed by a direct insert of the stone. Sub-actions can themselves contain sub-actions, recursively.

A useful mental model: each action's range covers the entire dependency tree of events and sub-actions occurring within it. A transaction is a sequence of one or more top-level actions; each top-level action's range covers all of its content at every nesting level.

```
[                top-level: MineStone               ]
   [   sub-action: UseWoodPick   ]   [insert stone ]
       [ mutate pick ]
```

## Validating changes via type guards

Once all action statements are proven, the transaction system "replays" the chain: walks it event by event, and for each event verifies that the affected object's change is authorized.

This is the role of the **type predicate** (`IsX`): every object's `type` field stores the hash of `Is<Class>`, which is an OR over the action predicates valid for that class. The type predicate takes the object state plus the current action's range, `(obj, chain_start, chain_end)`, as arguments. Validating an event means producing a statement of one of those OR branches whose range matches the current range. In a valid proof, that match is exactly one of the action statements proved at the start.

This is what ties the two halves together. The action statement commits to a range; the replay's guard call dispatches into IsX, which selects the right action; the action's recorded range must equal the range the guard is asking about. Each guard call has one and only one action statement that can satisfy it.

## The replay walker

Replay is structured as recursive OR-walking over the chain. Four layers, bottom up:

1. **Chain primitives.** `TxInsert`, `TxMutate`, `TxDelete` each witness one hash-step and pin the affected object's `type`. These primitives are referenced both by application action predicates at record time and by replay at finalize time, so the chain-step hash work is proved exactly once. They live in their own podlang module (`tx_events.podlang`, imported by `txlib.podlang`) so that their hashes -- which every plugin module and recorded transaction bakes in -- survive churn in the replay and finalize predicates.

2. **Replay.** Conceptually, replay walks the event tree and applies each step's state change and guard dispatch. In pseudocode:

   ```
   for each top-level action:
     replay(action)

   replay(action):
     scope = (action.chain_start, action.chain_end)
     for each step in action:
       if step is insert(new):      live.add(new);                      guard(new, scope)
       if step is mutate(old, new): live.swap(old, new); nullify(old);  guard(new, scope)
       if step is delete(old):      live.remove(old);    nullify(old);  guard(old, scope)
       if step is sub_action:       replay(sub_action)
   ```

   In podlang this is encoded as a recursive OR walk, which would be expensive if written naively: each step would OR over five cases at every iteration. Because the prover knows the event sequence in advance, the loop is unrolled and specialised: at each step the prover picks the right branch, and the variants of the next step are specialised to the type of the head event so the OR-dispatch over event types folds out. See `ReplayActions`, `ReplayContents`, and the per-event predicates `Replay{Insert,Mutate,Delete,Action}` for the details. `ReplayAction` also writes the scope into the tx context and copies inner `live`/`nullifiers` back to the outer tx. The top-level walker requires every top-level event to be an action, so no bare event can escape an action's guard dispatch.

   `ReplayActions` also has a K=1 fast path (`ReplayActionInsert`) for the common "mining" case: a single top-level action whose body is one Insert. It folds the whole walk into 2 custom statements by bypassing `ReplayAction` -> `ReplayContents` -> `ReplayElement` -> `ReplayInsert`, lifting the Insert's guard call directly under the top-level OR. This is safe because the action spans the full chain range in this case, so the action's `(chain_start, chain_end)` and the transaction's are the same values the guard would have seen inside a materialised `ReplayAction` scope.

3. **Grounding.** Each input object must be present in the global **created set** carried by the state root: a grow-only set holding the commitment of every object state ever created, maintained by the synchronizer. Grounding is one `ArrayContains(created, index, input)` membership check per input, with no indirection through a source transaction. `InputsGrounded` is an OR over `Equal(inputs, {})` (no inputs), `InputsGroundedSingle` (one input), `InputsGroundedPair` (two inputs, both grounded inline), and `InputsGroundedRecursive` (three or more: ground two inputs, then recurse, bottoming out at Single for odd counts or Pair for even). Since each input is only `ArrayContains` + `SetInsert`, Pair fits both inputs in one predicate and Recursive peels two per level. `created` is passed to `InputsGrounded` as a plain value so the membership checks cross POD boundaries cheaply; `TxFinalized` ties it back to the public state root's `created` field once.

   The created set is grow-only and grounding runs against a possibly-stale state root, so a grounded input may already have been spent. The nullifier set, not grounding, is what rejects a re-spend.

4. **`TxFinalized`.** The public entry point. It seeds the chain (`chain_start = H(live_set, {})`), pins the initial `before_tx` schema (`nullifiers = {}`, `chain_start = chain_end = {}`, `live = inputs_set`) in a single `DictInsert` clause to remove malleability, and threads everything through one `ReplayActions` call. `TxFinalBindings` surfaces the final `nullifiers` and `live` sets as public args (factored out so `TxFinalized` stays within the clause limit), letting the synchronizer fold them into its global nullifier and created sets. Public outputs: `(state_root, tx_final, nullifiers, live)`.
