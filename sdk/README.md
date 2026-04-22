# SDK

This is the Digital Objects SDK which contains the environment to define
classes of digital objects by their actions and execute them in a portable
manner.

# Architecture

The main interface of the SDK is a scripting language that is used to define
actions.  The current implementation uses [Rhai](https://rhai.rs) for that.
Each action is defined via a script function and a collection of actions define
a module which in turn define a collection of classes.

Action scripts are evaluated in two different phases:
- **Load**.  In this phase the scripting engine evaluates the code symbolically
  to extract the declaration of an action.
- **Execution**.  In this phase the scripting engine evaluates the code with
  real inputs to execute the action (which consumes, mutates and generates
  objects).

Both phases use the type `ActionHandle`, which contains a shared
`ActionContext` to track the state of evaluation.  `ActionHandle` offers a list
of host methods available in the script environment to define action
operations.

Action operations deal with literal values and runtime variable values.  The
`Ref` type contains a shared `VarOrValue` which allows treating literals and
variables uniformly.  The `Ref` type offers a list of host methods available in
the script environment to define value operations.  Operations will promote
native types compatible with pod2 values to `VarOrValue` automatically so no
type conversions are explicitly required on the scripting side.

In the Load phase, the `Loader` is used to collect the action declaration and
metadata.

In the Execution phase, the `Executor` is used to track the generated execution
artifacts.

## Literal values and Variable values

The action needs to be translated to a pod2 predicate which will use a mix of
literal embedded values and variables (called wildcards in the pod2 context).
In the scripting environment everything is stored in a scripting variable, but
we need to distinguish between the two cases.  For this reason we extend Rhai
with the following syntax: `var $ident$ = $expr$` for `var` declaration.  Any
expression not involving `var` will be evaluated at Load and Execute time.  A
declaration of a `var` will introduce it to the predicate scope.  Any
expression involving a `var` will be evaluated symbolically at Load time and
non-symbolically at Execute time.

## Unsafe expressions

By default all expressions that involve a `var` generate corresponding
statements that constrain the operation.  Sometimes this is not desirable
because we want to calculate the value of a `var` as a witness to some
statement.  The generation of constraining statements can be disabled by using
an `unsafe` block.

## Type checking

The scripting language has dynamic types so we do type checking at runtime.
Some level of type checking can be perfomed at Load time, but there are cases
where we can only do it at Execution time, like operations involving object
entries (at Load time we don't know what's the type of `pick.durability`).

## u256 difficulty targets

`pow_obj_grind(obj, target)` and `intro_lt_eq_u256(x, target)` both compare
full 256-bit `RawValue`s.  Integer literals in Rhai promote to a pod2 `Value`
whose `RawValue` has the integer in the *least*-significant limb — not what
you want for a "top-limb ≤ N" difficulty target.

Use `action.top_limb_u256(n)` to build a `RawValue` with `n` in the
most-significant limb and zeros elsewhere.  Bind it once with `let` (not
`var`, since it is a literal, not a wildcard) and reuse for both grinding
and the proof:

```rhai
let target = action.top_limb_u256(9007199254740992);
var key = action.pow_obj_grind(wood, target);
wood.update("key", key);
action.intro_lt_eq_u256(wood, target);
```

The emitted podlang embeds `target` as a hex `Raw(0x00…)` literal.

# Missing features

- [ ] Literal Array
  - [ ] get
  - [ ] insert
  - [ ] delete
  - [ ] update
- [ ] Literal Dictionary and operations
  - [ ] get
  - [ ] insert
  - [ ] delete
  - [ ] update
- [ ] Literal Set and operations
  - [ ] contains
  - [ ] insert
  - [ ] delete
- [ ] Var Array
  - [ ] get
  - [ ] insert
  - [ ] delete
  - [ ] update
- [ ] Var Dictionary/Object and operations
  - [ ] get
  - [ ] insert
  - [ ] delete
  - [x] update
  - [x] set
- [ ] Var Set and operations
  - [ ] contains
  - [ ] insert
  - [ ] delete
- [ ] Statements:
    - [ ] Equal
    - [ ] NotEqual
    - [ ] LtEq
    - [ ] Lt
    - [ ] Contains
    - [ ] NotContains
    - [x] SumOf
    - [ ] ProductOf
    - [ ] MaxOf
    - [ ] HashOf
    - [ ] PublicKeyOf
    - [ ] SignedBy
    - [ ] ContainerInsert
    - [ ] ContainerUpdate
    - [ ] ContainerDelete
    - [ ] DictContains
    - [ ] DictNotContains
    - [ ] SetContains
    - [ ] SetNotContains
    - [ ] ArrayContains
    - [ ] GtEq
    - [x] Gt
    - [ ] DictInsert
    - [ ] DictUpdate
    - [ ] DictDelete
    - [ ] SetInsert
    - [ ] SetDelete
    - [ ] ArrayUpdate
- [ ] Execution time type checking without panics
- [ ] operator+
- [ ] operator*
- [x] dependent action
- [x] pexe.zip support (packaged by the `pexe` crate's CLI)
- [x] manifest support
- [ ] error pretty print
- [ ] forbid multiple Object::set operations on the same object
- [ ] forbid Object::set after the objec thas been used in other operations

# Test example

The example in the test `test_sdk_1` produces the following podlang code:

```
use module 0x94ac3a36e50a41d0ad361b0b3eaabb335212fac001ea13dcccc6db40183e2551 as tx
use intro Vdf(count, input, output) from 0xb77a964de74c8569e6c6172692bb50147df9334fd9b572abc8d4d9c688a40e06
use intro LtEqU256(lhs, rhs) from 0x2e79114ee823f4783ab5b6eb93b49abba87fb69b4d14de4cf1d78648ade73529

// Actions

FindLog(log, tx, tx0, private: log0, work) = AND(
  DictContains(log0, "blueprint", "Log")
  Vdf(3, log0, work)
  DictUpdate(log, log0, "work", work)
  tx::TxInserted(tx, tx0, log)
)

CraftWood(wood, tx, tx0, private: tx1, log, wood0, key) = AND(
  IsLog(log)
  DictContains(wood0, "blueprint", "Wood")
  DictUpdate(wood, wood0, "key", key)
  LtEqU256(wood, Raw(0x0020000000000000000000000000000000000000000000000000000000000000))
  tx::TxDeleted(tx1, tx0, log)
  tx::TxInserted(tx, tx1, wood)
)

CraftSticks(stick_a, stick_b, tx, tx0, private: tx1, tx2, wood) = AND(
  IsWood(wood)
  DictContains(stick_a, "blueprint", "Stick")
  DictContains(stick_b, "blueprint", "Stick")
  tx::TxDeleted(tx1, tx0, wood)
  tx::TxInserted(tx2, tx1, stick_a)
  tx::TxInserted(tx, tx2, stick_b)
)

CraftWoodPick(pick, tx, tx0, private: tx1, tx2, wood, stick) = AND(
  IsWood(wood)
  IsStick(stick)
  DictContains(pick, "blueprint", "WoodPick")
  DictContains(pick, "durability", 100)
  tx::TxDeleted(tx1, tx0, wood)
  tx::TxDeleted(tx2, tx1, stick)
  tx::TxInserted(tx, tx2, pick)
)

UseWoodPick(wood_pick, tx, tx0, private: wood_pick0, wood_pick1, wood_pick2, durability, key, work) = AND(
  IsWoodPick(wood_pick0)
  Gt(wood_pick0.durability, 0)
  SumOf(wood_pick0.durability, durability, 1)
  DictUpdate(wood_pick1, wood_pick0, "durability", durability)
  DictUpdate(wood_pick2, wood_pick1, "key", key)
  Vdf(10, wood_pick2, work)
  DictUpdate(wood_pick, wood_pick2, "work", work)
  tx::TxMutated(tx, tx0, wood_pick, wood_pick0)
)

MineStoneWithWoodPick(stone, tx, tx0, private: tx1, pick) = AND(
  UseWoodPick(pick, tx1, tx0)
  DictContains(stone, "blueprint", "Stone")
  tx::TxInserted(tx, tx1, stone)
)

// Classes

IsLog(state, private: tx, tx0) = OR(
  FindLog(state, tx, tx0)
)

IsWood(state, private: tx, tx0) = OR(
  CraftWood(state, tx, tx0)
)

IsStick(state, private: tx, tx0, _other_0) = OR(
  CraftSticks(state, _other_0, tx, tx0)
  CraftSticks(_other_0, state, tx, tx0)
)

IsWoodPick(state, private: tx, tx0) = OR(
  CraftWoodPick(state, tx, tx0)
  UseWoodPick(state, tx, tx0)
)

IsStone(state, private: tx, tx0) = OR(
  MineStoneWithWoodPick(state, tx, tx0)
)
```
