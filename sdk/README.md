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
use module 0xb1a79d9f0953e49fdcf43697c3c61aa5b59b2b88f5a5dcde8b8c76942dfdd4f4 as tx
use intro Vdf(count, input, output) from 0xb77a964de74c8569e6c6172692bb50147df9334fd9b572abc8d4d9c688a40e06
use intro LtEqU256(lhs, rhs) from 0x2e79114ee823f4783ab5b6eb93b49abba87fb69b4d14de4cf1d78648ade73529

record FindLogOut = (_pad, log)
record CraftWoodIn = (_pad, log)
record CraftWoodOut = (_pad, wood)
record CraftSticksIn = (_pad, wood)
record CraftSticksOut = (_pad, stick_a, stick_b)
record CraftSticksChain = (_pad, step_0, step_1)
record CraftWoodPickIn = (_pad, wood, stick)
record CraftWoodPickOut = (_pad, pick)
record CraftWoodPickChain = (_pad, step_0, step_1)
record UseWoodPickIn = (_pad, wood_pick)
record UseWoodPickOut = (_pad, wood_pick)
record MineStoneWithWoodPickOut = (_pad, stone)

// Actions

FindLog(out FindLogOut, chain0, chain, private: log0, work) = AND(
  Vdf(3, log0, work)
  DictUpdate(out.log, log0, "work", work)
  tx::TxInsert(chain, chain0, out.log, @self_predicate(IsLog))
)

CraftWood(in CraftWoodIn, out CraftWoodOut, chain0, chain, private: chain1, wood0, wood, key) = AND(
  ArrayContains(out, CraftWoodOut::wood, wood)
  DictUpdate(wood, wood0, "key", key)
  LtEqU256(wood, Raw(0x0020000000000000000000000000000000000000000000000000000000000000))
  tx::TxDelete(chain1, chain0, in.log, @self_predicate(IsLog))
  tx::TxInsert(chain, chain1, wood, @self_predicate(IsWood))
)

CraftSticks(in CraftSticksIn, out CraftSticksOut, chain0, chain, private: chain_steps CraftSticksChain) = AND(
  tx::TxDelete(chain_steps.step_0, chain0, in.wood, @self_predicate(IsWood))
  tx::TxInsert(chain_steps.step_1, chain_steps.step_0, out.stick_a, @self_predicate(IsStick))
  tx::TxInsert(chain, chain_steps.step_1, out.stick_b, @self_predicate(IsStick))
)

CraftWoodPick(in CraftWoodPickIn, out CraftWoodPickOut, chain0, chain, private: chain_steps CraftWoodPickChain) = AND(
  DictContains(out.pick, "durability", 100)
  tx::TxDelete(chain_steps.step_0, chain0, in.wood, @self_predicate(IsWood))
  tx::TxDelete(chain_steps.step_1, chain_steps.step_0, in.stick, @self_predicate(IsStick))
  tx::TxInsert(chain, chain_steps.step_1, out.pick, @self_predicate(IsWoodPick))
)

UseWoodPick(in UseWoodPickIn, out UseWoodPickOut, chain0, chain, private: wood_pick0, wood_pick1, wood_pick2, durability, key, work) = AND(
  ArrayContains(in, UseWoodPickIn::wood_pick, wood_pick0)
  Gt(wood_pick0.durability, 0)
  SumOf(wood_pick0.durability, durability, 1)
  DictUpdate(wood_pick1, wood_pick0, "durability", durability)
  DictUpdate(wood_pick2, wood_pick1, "key", key)
  Vdf(10, wood_pick2, work)
  DictUpdate(out.wood_pick, wood_pick2, "work", work)
  tx::TxMutate(chain, chain0, out.wood_pick, wood_pick0, @self_predicate(IsWoodPick))
)

MineStoneWithWoodPick(out MineStoneWithWoodPickOut, chain0, chain, private: chain1, _UseWoodPick_in_0 UseWoodPickIn, _UseWoodPick_out_0 UseWoodPickOut) = AND(
  UseWoodPick(_UseWoodPick_in_0, _UseWoodPick_out_0, chain0, chain1)
  tx::TxInsert(chain, chain1, out.stone, @self_predicate(IsStone))
)

// Bridges

IsLogFromFindLog(state, chain0, chain, private: out FindLogOut) = AND(
  ArrayContains(out, FindLogOut::log, state)
  FindLog(out, chain0, chain)
)

IsLogFromCraftWood(state, chain0, chain, private: in CraftWoodIn, out CraftWoodOut) = AND(
  ArrayContains(in, CraftWoodIn::log, state)
  CraftWood(in, out, chain0, chain)
)

IsWoodFromCraftWood(state, chain0, chain, private: in CraftWoodIn, out CraftWoodOut) = AND(
  ArrayContains(out, CraftWoodOut::wood, state)
  CraftWood(in, out, chain0, chain)
)

IsWoodFromCraftSticks(state, chain0, chain, private: in CraftSticksIn, out CraftSticksOut) = AND(
  ArrayContains(in, CraftSticksIn::wood, state)
  CraftSticks(in, out, chain0, chain)
)

IsStickFromCraftSticks_stick_a(state, chain0, chain, private: in CraftSticksIn, out CraftSticksOut) = AND(
  ArrayContains(out, CraftSticksOut::stick_a, state)
  CraftSticks(in, out, chain0, chain)
)

IsStickFromCraftSticks_stick_b(state, chain0, chain, private: in CraftSticksIn, out CraftSticksOut) = AND(
  ArrayContains(out, CraftSticksOut::stick_b, state)
  CraftSticks(in, out, chain0, chain)
)

IsWoodFromCraftWoodPick(state, chain0, chain, private: in CraftWoodPickIn, out CraftWoodPickOut) = AND(
  ArrayContains(in, CraftWoodPickIn::wood, state)
  CraftWoodPick(in, out, chain0, chain)
)

IsStickFromCraftWoodPick(state, chain0, chain, private: in CraftWoodPickIn, out CraftWoodPickOut) = AND(
  ArrayContains(in, CraftWoodPickIn::stick, state)
  CraftWoodPick(in, out, chain0, chain)
)

IsWoodPickFromCraftWoodPick(state, chain0, chain, private: in CraftWoodPickIn, out CraftWoodPickOut) = AND(
  ArrayContains(out, CraftWoodPickOut::pick, state)
  CraftWoodPick(in, out, chain0, chain)
)

IsWoodPickFromUseWoodPick(state, chain0, chain, private: in UseWoodPickIn, out UseWoodPickOut) = AND(
  ArrayContains(out, UseWoodPickOut::wood_pick, state)
  UseWoodPick(in, out, chain0, chain)
)

IsStoneFromMineStoneWithWoodPick(state, chain0, chain, private: out MineStoneWithWoodPickOut) = AND(
  ArrayContains(out, MineStoneWithWoodPickOut::stone, state)
  MineStoneWithWoodPick(out, chain0, chain)
)

// Classes

IsLog(state, chain0, chain) = OR(
  IsLogFromFindLog(state, chain0, chain)
  IsLogFromCraftWood(state, chain0, chain)
)

IsWood(state, chain0, chain) = OR(
  IsWoodFromCraftWood(state, chain0, chain)
  IsWoodFromCraftSticks(state, chain0, chain)
  IsWoodFromCraftWoodPick(state, chain0, chain)
)

IsStick(state, chain0, chain) = OR(
  IsStickFromCraftSticks_stick_a(state, chain0, chain)
  IsStickFromCraftSticks_stick_b(state, chain0, chain)
  IsStickFromCraftWoodPick(state, chain0, chain)
)

IsWoodPick(state, chain0, chain) = OR(
  IsWoodPickFromCraftWoodPick(state, chain0, chain)
  IsWoodPickFromUseWoodPick(state, chain0, chain)
)

IsStone(state, chain0, chain) = OR(
  IsStoneFromMineStoneWithWoodPick(state, chain0, chain)
)
```
