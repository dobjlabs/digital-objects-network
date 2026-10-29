# Digital Object Lifecycle

A transaction publishes commitments to every object state it creates, including intermediate states that it later mutates or deletes. It also publishes nullifiers for every state it consumes. A published state is spendable only while its nullifier is absent from the synchronizer's nullifier set.

## 1. Creation

When an action creates an object, it:

1. Builds a dictionary containing its fields, including `key` and `type`.
2. Records a `TxInsert` event and assigns the object's `stable_identifier`.
3. Proves that the creation is authorized by the object's class predicate.
4. Adds the state to the transaction's created set.

The transaction payload includes the state's commitment even if a later event consumes it. Only surviving outputs are returned as spendable objects. After confirmation, the driver marks an output `live` if its commitment is published and its nullifier is absent.

A `.dobj` file stores the object dictionary (`obj`) and a `metadata` record: its class, content hash, status, and producing transaction's commitment. Once the object is confirmed, `metadata.creationProof` holds a Merkle proof that its commitment is in the created set of a state root, which import checks against the synchronizer. The file does not contain the transaction proof; that proof is submitted with the transaction payload.

## 2. Inspection

Object inspection exposes:

- **contentHash**: the commitment identifying this object state.
- **fileName**: the local `.dobj` filename.
- **class** and **classHash**: the object's class identity.
- **fields**: application fields such as durability and key.
- **status**: `unknown`, `pending`, `live`, or `nullified`.

Class inspection provides the predicate source describing authorized transitions. Objects awaiting confirmation may be `unknown` or `pending`; only `live` objects are eligible as action inputs.

## 3. Mutation

Mutation consumes an old state and creates a replacement state. For example, using a pickaxe can decrement its durability:

1. The action records a `TxMutate` event, preserving the object's class and `stable_identifier`.
2. Replay verifies the class guard, adds the new state to the created set, and nullifies the old state.
3. Once confirmed, the old state is `nullified`. The replacement is `live` if no later event consumed it.

The states have different commitments. If several mutations occur in one transaction, all intermediate commitments are published and all consumed intermediate states are nullified. Returning to an already-created commitment is rejected.

## 4. Consumption

Deletion records a `TxDelete` event, verifies the object's class guard, and publishes its nullifier without creating a replacement state. It never removes the object's commitment from the global created array.

Every declared transaction input must be consumed by mutation or deletion. An action that only reads an input must still consume and reissue it if the object should remain spendable.

## 5. Nullifiers and double-spend prevention

A state's nullifier is `hash(hash(obj_dict, obj_dict.key), "txlib-nullifier-v1")`. The synchronizer maintains the global nullifier set and rejects transactions containing an already-published nullifier. Replay also prevents consuming the same state twice within a transaction.

The published payload contains commitments and nullifiers, not the private object dictionaries. Linking a nullifier to its object requires the private object data used to derive it.

## 6. State root grounding

A transaction references a recent state header committing to the created array, nullifier set, prior state history, and block metadata. `InputsGrounded` proves that each input existed in the referenced created array. Every input must also be consumed, so the transaction publishes its nullifier.

Historical creation membership alone does not prove that an input is still live. The synchronizer checks the emitted nullifiers against its current state, rejecting inputs spent since the grounding root. It also rejects grounding roots more than 300 blocks old.

## Transaction results

| State | Published commitment | Published nullifier | Spendable after confirmation |
| --- | --- | --- | --- |
| External input consumed by mutation or deletion | Already published | Yes | No |
| Created state retained as an output | Yes | No | Yes |
| Created state mutated or deleted later in the transaction | Yes | Yes | No |
