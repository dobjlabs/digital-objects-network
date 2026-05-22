# Digital Object Lifecycle

This document walks through the lifecycle of a Digital Object from creation to consumption, showing what happens at each stage.

## 1. Creation

When an action creates a new object, it:

1. Builds a fresh dictionary with the object's fields (e.g. `{key: 0xabc, blueprint: "Ingot"}`).
2. Generates a ZK proof that this dictionary satisfies the class predicate (e.g. `IsIngot(state) = AND(CraftIngot(state, iron))`). The proof includes verification that the input `iron` was itself valid.
3. Records the new object in a transaction via `TxInserted`, adding it to the transaction's live set.
4. The object appears in inventory as `live: true` with its fields visible.

The resulting `.dobj` file contains the state dictionary and the proof. The proof is constant-size — it does not grow as objects are created from longer chains of prior objects.

## 2. Inspection

When you inspect a live object, you see:
- **id**: a hash uniquely identifying this object state
- **className**: determined by the predicate that certifies it
- **fields**: the key-value pairs (blueprint, durability, key, etc.)
- **predicateSource**: the podlang rule showing all valid transitions
- **live**: true, meaning its nullifier has not been published

## 3. Mutation

Some actions mutate objects rather than consuming them (e.g. using a DrillBit decrements its durability, touching a MachineII proves possession without spending it). A mutation:

1. Proves the current state is valid.
2. Modifies fields (e.g. `DictUpdate(new, old, "durability", new_value)`).
3. Publishes a nullifier for the OLD state (preventing it from being reused).
4. Records the transition via `TxMutated(tx_after, tx_before, new, old)`.
5. The old object becomes `live: false`. The new object appears as `live: true` with updated fields.

The old object ID and the new object ID are different (the ID is a hash of the state dictionary, and the fields changed).

## 4. Consumption

When an object is consumed as input to an action (e.g. an Iron consumed by CraftIngot):

1. The action proves the input is valid.
2. A nullifier is published for the input object.
3. The input is removed from the live set via `TxDeleted`.
4. The input object becomes `live: false` permanently.
5. Output objects are created as in step 1.

## 5. Nullifiers and double-spend prevention

A nullifier is a hash derived from the object's key: `hash(hash(obj_dict, obj_dict.key), "txlib-nullifier-v1")`. It is published to a global set tracked by the synchronizer.

Once published, any attempt to use the same object in another action will fail because the nullifier is already in the set. This prevents double-spending without revealing which specific object was spent (the nullifier is a hash, not the object itself).

## 6. State root grounding

Every action must reference a recent Global State Root (GSR) — a hash of all published transactions and nullifiers. This ensures the action's inputs were live at a known point in time. The synchronizer rejects actions grounded in a GSR more than ~300 blocks (~1 hour) old.

## Summary of what changes after each action

| Before | Action | After |
|--------|--------|-------|
| Input objects: `live: true` | Action runs | Input objects: `live: false` (nullified) |
| — | Proof generated | Output objects: `live: true` (new) |
| Mutated objects: `live: true` | Action runs | Old state: `live: false`, New state: `live: true` |
