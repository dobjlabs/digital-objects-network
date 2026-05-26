# txlib

Transaction predicates for verifiable state transitions in the Digital Objects Network.

## SDK: actions and classes

The SDK defines **actions** and **classes**. A class is a label for an object's type - what kind of thing it is. Some actions output objects of class "Wood"; others take "Wood" as input. The class is defined by the set of actions capable of producing, mutating, or deleting an object of that class. Concretely, every object dictionary carries a `type` field whose value is the hash of an `Is<Class>` predicate. That predicate is an OR over all the actions in the class.

## Transactions: verifying state changes

A transaction verifies a set of state changes -- newly created object states and nullifications of existing object states. It checks both that the initial states are valid (grounded in the prior global state) and that every change -- creation of a new object, mutation or deletion of an existing one -- has a valid cause.

For this to work, actions need a way of saying "these are the changes I caused", and the system needs a way to look at any change and ask "is there a valid cause for this to have happened?" — i.e. "did this change originate in one of the permitted actions for the affected object's class?"

## Action statements

Before the system can answer that question, the prover first produces **action statements**: proofs of action predicates like `FindLog`, `CraftWood`, `UseWoodPick`. Each action statement commits to:

- The set of inputs the action consumed (possibly empty -- `FindLog` takes none).
- The set of outputs the action produced (possibly empty -- a pure-delete action produces none).
- A range on the transaction's hash chain (see next).

## The hash chain

Every event in a transaction -- insert, mutate, delete -- is recorded as one step of a chained hash. For each event a small per-event hash is computed (insert: `H(0, new)`; mutate: `H(old, new)`; delete: `H(old, 0)`) and folded into the running chain (`chain = H(prev_chain, event_hash)`). The chain is the canonical, ordering-preserving commitment to "what happened, in what order".

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

This is the role of the **type predicate** (`IsX`): every object's `type` field stores the hash of `Is<Class>`, which is an OR over the action predicates valid for that class. The type predicate takes `(obj, chain_start, chain_end)` -- the object state plus the current action's range -- as arguments. Validating an event means producing a statement of one of those OR branches whose range matches the current range. In a valid proof, that match is exactly one of the action statements proved at the start.

This is what ties the two halves together. The action statement commits to a range; the replay's guard call dispatches into IsX, which selects the right action; the action's recorded range must equal the range the guard is asking about. Each guard call has one and only one action statement that can satisfy it.

## The replay walker

Replay is structured as recursive OR-walking over the chain. Four layers, bottom up:

1. **Chain primitives.** `TxInsert`, `TxMutate`, `TxDelete` each witness one hash-step and pin the affected object's `type`. These primitives are referenced both by application action predicates at record time and by replay at finalize time, so the chain-step hash work is proved exactly once.

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

   In podlang this is encoded as a recursive OR walk, which would be expensive if written naively: each step would OR over five cases at every iteration. Because the prover knows the event sequence in advance, the loop can be unrolled and specialised -- at each step the prover picks the right branch, and the variants of the next step are specialised to the type of the head event so the OR-dispatch over event types folds out. See `ReplayActions`, `ReplayContents`, and the per-event predicates `Replay{Insert,Mutate,Delete,Action}` for the details. `ReplayAction` also writes the scope into the tx context and copies inner `live`/`nullifiers` back to the outer tx. The top-level walker requires every top-level event to be an action, so no bare event can escape an action's guard dispatch.

   `ReplayActions` also has a K=1 fast path (`ReplayActionInsert`) for the common "mining" case: a single top-level action whose body is one Insert. It folds the whole walk into 2 custom statements by bypassing `ReplayAction` -> `ReplayContents` -> `ReplayElement` -> `ReplayInsert`, lifting the Insert's guard call directly under the top-level OR. Safe because the action spans the full chain range in this case, so the action's `(chain_start, chain_end)` and the transaction's are the same values the guard would have seen inside a materialised `ReplayAction` scope.

3. **Grounding.** Each input object must be live in some prior finalized tx that's recorded in the state root. `InputsGrounded` is an OR with fast paths for 0/1/2/3 inputs (`Equal({},{})`, `Single`, `Pair`, `Triple`) plus a recursive branch for 4+; the fast paths avoid the per-input cost of the general recursion. `TxInStateRoot` unpacks the 3-layer state-root hash inline rather than calling a separate `StateRoot` predicate.

4. **`TxFinalized`.** The public entry point. It seeds the chain (`chain_start = H(live_set, 0)`), pins the initial `before_tx` schema (`nullifiers = {}`, `chain_start = chain_end = 0`, `live = inputs_set`) in a single `DictInsert` clause to remove malleability, and threads everything through one `ReplayActions` call. Public outputs: `(state_root_hash, tx_final, nullifiers)`.
