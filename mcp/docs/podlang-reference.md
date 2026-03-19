# Podlang Reference

Podlang is a declarative language for specifying zero-knowledge proof constraints in the POD2 framework. Predicates define what must be true for a proof to verify.

## Syntax

A predicate definition has the form:

```
PredicateName(public_arg1, public_arg2, private: priv_arg1, priv_arg2) = COMBINER(
  Clause1(...)
  Clause2(...)
)
```

- **Public arguments** (before `private:`) are visible to the verifier. They appear in the proof's public inputs and can be constrained by callers.
- **Private arguments** (after `private:`) are witnesses known only to the prover. They are consumed during proof generation and never revealed.
- If there are no private arguments, the `private:` keyword is omitted.

Comments use `// line comment` or `/* block comment */` syntax.

Identifiers (predicate names, argument names) must start with a letter or `_`, followed by letters, digits, or `_`. The words `private`, `true`, and `false` are reserved.

## Combiners

- `AND(...)` — all clauses must hold simultaneously.
- `OR(...)` — exactly one branch must hold. Used for the state-machine pattern where an object could have been produced by any of several actions.

## Literal values

- **Integer**: `0`, `42`, `-1` (64-bit signed)
- **Boolean**: `true`, `false`
- **String**: `"hello"`, `"escaped \" quote"`
- **Raw (32-byte hex)**: `Raw(0x...64 hex digits...)`
- **Public key**: `PublicKey(base58string)`
- **Secret key**: `SecretKey(base64string)`
- **Dictionary**: `{"key": value, ...}` or `{}` for empty. Keys must be strings.
- **Set**: `#[element, ...]` or `#[]` for empty
- **Array**: `[element, ...]` or `[]` for empty
- **Predicate literal**: `::PredicateName` or `module::PredicateName` (resolves to the predicate's identity hash)

All value types can be hashed (producing a 256-bit Poseidon hash). Dictionaries, arrays, and sets are collectively called "containers" and are implemented as Merkle trees, identified by their root hash.

## Built-in predicates

These are the predicates available in podlang. Some are native to the backend circuit; others are syntactic sugar that the compiler lowers to native operations.

### Dictionary operations
- `DictContains(dict, "key", value)` — the dictionary contains this key-value pair.
- `DictNotContains(dict, "key")` — the dictionary does NOT contain this key.
- `DictInsert(new, old, "key", value)` — `new` equals `old` with `key` added. Key must NOT exist in `old`.
- `DictUpdate(new, old, "key", value)` — `new` equals `old` with `key`'s value changed. Key MUST already exist in `old`.
- `DictDelete(new, old, "key")` — `new` equals `old` with `key` removed. Key MUST exist in `old`.

### Set operations
- `SetContains(set, element)` — the set contains this element.
- `SetNotContains(set, element)` — the set does NOT contain this element.
- `SetInsert(new, old, element)` — `new` equals `old` with element added.
- `SetDelete(new, old, element)` — `new` equals `old` with element removed.

### Array operations
- `ArrayContains(array, index, element)` — the array contains element at index.
- `ArrayUpdate(new, old, index, value)` — `new` equals `old` with value at index changed.

### Equality and comparison
- `Equal(a, b)` — `a` equals `b` (works on any type; for compound types, compares hashes).
- `NotEqual(a, b)` — `a` does not equal `b`.
- `Gt(a, b)` — `a > b` (integers only).
- `GtEq(a, b)` — `a >= b` (integers only).
- `Lt(a, b)` — `a < b` (integers only).
- `LtEq(a, b)` — `a <= b` (integers only).

### Arithmetic
- `SumOf(sum, a, b)` — `sum = a + b`.
- `ProductOf(product, a, b)` — `product = a * b`.
- `MaxOf(max, a, b)` — `max = max(a, b)`.

### Hashing
- `HashOf(hash, input1, input2)` — `hash` is the Poseidon hash of the two inputs.

### Cryptographic
- `PublicKeyOf(pubkey, secret)` — `pubkey` is the public key derived from `secret`.
- `SignedBy(message, pubkey)` — `message` is signed by the holder of `pubkey`.

## Statement arguments

Each argument to a predicate clause can be:

- A **variable name** — bound by the predicate's public or private args
- A **literal value** — an integer, string, boolean, empty dict `{}`, empty set `#[]`, etc.
- An **anchored key** — a reference into a dictionary (see below)

## Anchored keys

An anchored key is a reference to a value inside a dictionary. There are two notations:

- **Dot notation**: `state.field` — only valid for identifier-like keys
- **Bracket notation**: `state["field"]` — allows any string key

These work in built-in predicate clauses:

```
GtEq(distance, old_state.locked)
DictContains(pod["name"], "Alice")
```

But anchored keys are **NOT allowed as arguments to a sub-predicate call**:

```
// WRONG — anchored key passed to sub-predicate:
NotExpired(state.timeout_block, ...)

// CORRECT — extract first via DictContains, then pass:
MyPred(state, private: timeout) = AND(
  DictContains(state, "timeout_block", timeout)
  NotExpired(timeout, ...)
)
```

## The IsX state-machine pattern

Every object class has a top-level predicate following this pattern:

```
IsClassName(state) = OR(
  CreateAction(state, ...)
  MutateAction(state, prev_state)
  AnotherAction(state, ...)
)
```

Each OR branch represents one valid way the object could have reached its current state. When proving `IsClassName(state)`, the prover supplies the private witness for whichever branch actually applies. The verifier learns only that *some* valid branch holds — not which one.

Private args of the OR are the union of all branches' private args.

## Imports

### Module import

Imports a predicate group (identified by its Merkle root hash) and gives it a local alias:

```
use module 0xHASH as txlib

MyPred(...) = AND(
  txlib::StateRoot(...)
)
```

Multiple predicates can be defined in a single group (file), and they can reference each other — including recursively.

### Introduction pod import

Imports a cryptographic primitive (introduction predicate) from an introduction pod:

```
use intro predicate_name(arg1, arg2) from 0xHASH
```

Introduction pods provide base-level facts (like signatures or VDF results) that don't come from custom predicates.

## Recursive predicates

Custom predicates can reference themselves or other predicates in the same group. This enables inductive definitions:

```
eth_dos_base(src, dst, distance) = AND(
  Equal(src, dst)
  Equal(distance, 0)
)

eth_dos_ind(src, dst, distance, private: shorter_distance, mid) = AND(
  eth_dos(src, mid, shorter_distance)
  SumOf(distance, shorter_distance, 1)
  eth_friend(mid, dst)
)

eth_dos(src, dst, distance) = OR(
  eth_dos_base(src, dst, distance)
  eth_dos_ind(src, dst, distance)
)
```

## Common patterns

### Creating an object
A create action typically initializes a fresh dictionary with `DictInsert` operations, then wraps it in a transaction via `TxInserted`.

### Mutating an object
A mutation proves the old state is valid (`IsClassName(old_state)`), modifies fields with `DictUpdate`, and records the transition via `TxMutated`. The old object gets a nullifier to prevent reuse.

### Consuming an object
Consumption proves validity of the input, then removes it via `TxDeleted`. The nullifier is published, permanently marking the object as spent.
