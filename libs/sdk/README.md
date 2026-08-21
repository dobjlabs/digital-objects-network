# SDK

This is the Digital Objects SDK which contains the environment to define
classes of digital objects by their actions and execute them in a portable
manner.

# Architecture

The main interface of the SDK is a scripting language that is used to define
actions. The current implementation uses [Rhai](https://rhai.rs) for that.
Each action is defined via a script function and a collection of actions define
a module which in turn define a collection of classes.

Action scripts are evaluated in two different phases:

- **Load**. In this phase the scripting engine evaluates the code symbolically
  to extract the declaration of an action.
- **Execution**. In this phase the scripting engine evaluates the code with
  real inputs to execute the action (which consumes, mutates and generates
  objects).

Both phases use the type `ActionHandle`, which contains a shared
`ActionContext` to track the state of evaluation. `ActionHandle` offers a list
of host methods available in the script environment to define action
operations.

Action operations deal with literal values and runtime variable values. The
`Ref` type contains a shared `VarOrValue` which allows treating literals and
variables uniformly. The `Ref` type offers a list of host methods available in
the script environment to define value operations. Operations will promote
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
we need to distinguish between the two cases. For this reason we extend Rhai
with the following syntax: `var $ident$ = $expr$` for `var` declaration. Any
expression not involving `var` will be evaluated at Load and Execute time. A
declaration of a `var` will introduce it to the predicate scope. Any
expression involving a `var` will be evaluated symbolically at Load time and
non-symbolically at Execute time.

## Unsafe expressions

By default all expressions that involve a `var` generate corresponding
statements that constrain the operation. Sometimes this is not desirable
because we want to calculate the value of a `var` as a witness to some
statement. The generation of constraining statements can be disabled by using
an `unsafe` block.

The integer operators `+`, `-` and `*` on `var` values are only available
inside `unsafe` blocks: they compute a witness value and emit nothing. Pair the
result with an explicit statement (`action.st_sum`, `action.st_product`,
`action.st_gt`, ...) afterward, or a malicious prover can substitute any value.
For `-` the pairing is a `Sum` with the operands rearranged (`a - b == r` is
stated as `r + b == a`), since pod2 has no subtraction predicate.

They stay `unsafe`-only on purpose rather than emitting their own statement
outside a block. `unsafe` applies to the dynamic extent of its block, so it
reaches into any script function called from inside one; an operator whose
meaning depended on that would constrain its result or not according to the
caller, and the unconstrained reading is the silent one.

## Native statements

`action.st_*` emits one pod2 native statement. Arguments are in the
predicate's own order, so a call reads the same as the podlang it renders to,
and each one takes a literal, a `var`, or a field read (`obj.field`). A
whole-container argument naming an object is anchored to its record entry
automatically.

| Call | Holds when |
| --- | --- |
| `st_equal(a, b)`, `st_not_equal(a, b)` | the two values are (not) equal; any pod2 values |
| `st_lt(a, b)`, `st_lt_eq(a, b)`, `st_gt(a, b)`, `st_gt_eq(a, b)` | integer comparison |
| `st_sum(a, b, c)` | `a + b == c` |
| `st_product(a, b, c)` | `a * b == c` |
| `st_max(a, b, c)` | `max(a, b) == c` |
| `st_hash(a, b, c)` | `c` is the pod2 hash of `a` and `b` |
| `st_contains(c, k, v)`, `st_not_contains(c, k)` | any container (does not) hold `k` (mapped to `v`) |
| `st_dict_contains(d, k, v)`, `st_dict_not_contains(d, k)` | same, and `d` is a dictionary |
| `st_set_contains(s, v)`, `st_set_not_contains(s, v)` | `s` is a set that does (not) hold `v` |
| `st_array_contains(a, i, v)` | array `a` holds `v` at index `i` |
| `st_container_insert(old, k, v, new)` | `new` is `old` with `(k, v)` added |
| `st_container_update(old, k, v, new)` | `new` is `old` with `k` remapped to `v` |
| `st_container_delete(old, k, new)` | `new` is `old` with `k` removed |
| `st_dict_insert`, `st_dict_update`, `st_dict_delete` | same three, pinning the container to a dictionary |
| `st_set_insert(old, v, new)`, `st_set_delete(old, v, new)` | `new` is `old` with `v` added / removed |
| `st_array_update(old, i, v, new)` | `new` is `old` with index `i` set to `v` |

Two limits are worth knowing before reaching for these:

- The transition statements (`st_*_insert` / `_update` / `_delete`) constrain
  a relation between two container values. They do not compute the new
  container, so it has to come from somewhere else: an object's entry, or a
  witness from an `unsafe` block. Until the SDK can build container values
  (see Missing features), the reachable use is relating two containers a
  script already holds.
- A field read of an object's whole-dict form is fine on inputs and mutates,
  but reading a field of an *output* you just built (`out.durability`) is not
  supported: the emitter renders it as `initials.out.durability`, a double
  anchor podlang has no syntax for, and the module fails to compile.

## Type checking

The scripting language has dynamic types so we do type checking at runtime.
Some level of type checking can be perfomed at Load time, but there are cases
where we can only do it at Execution time, like operations involving object
entries (at Load time we don't know what's the type of `pick.durability`).

## u256 difficulty targets

`pow_obj_grind(obj, target)` and `intro_lt_eq_u256(x, target)` both compare
full 256-bit `RawValue`s. Integer literals in Rhai promote to a pod2 `Value`
whose `RawValue` has the integer in the _least_-significant limb — not what
you want for a "top-limb ≤ N" difficulty target.

Use `action.top_limb_u256(n)` to build a `RawValue` with `n` in the
most-significant limb and zeros elsewhere. Bind it once with `let` (not
`var`, since it is a literal, not a wildcard) and reuse for both grinding
and the proof:

```rhai
let target = action.top_limb_u256(9007199254740992);
var key = action.pow_obj_grind(wood, target);
wood.update("key", key);
action.intro_lt_eq_u256(wood, target);
```

The emitted podlang embeds `target` as a hex `Raw(0x00…)` literal.

## Reading the state header

Every action scope has a `state_header` constant: the `StateHeader` record of
the state root the transaction grounds against. Its entries are
`block_number`, `block_timestamp`, `block_hash`, `created`, `nullifiers` and
`prior_state_history`. A field access like `state_header.block_timestamp`
behaves like a `var` entry ref: using it in a statement or an object write
emits an anchored statement against the action's public `state_header`
argument, which txlib pins to the grounded state root all the way up from
`TxFinalized` — a prover cannot substitute its own values.

```rhai
fn Tick(action) {
    var ticker = action.mutate("Ticker");
    var min_ts = unsafe { ticker.ts + 3600 };
    action.st_sum(ticker.ts, 3600, min_ts);
    action.st_gt(state_header.block_timestamp, min_ts);
    ticker.update("ts", state_header.block_timestamp);
}
```

Timing caveat: the header describes the _grounding_ state root, not the block
that will include the transaction. The synchronizer accepts grounding roots up
to `MAX_STATE_ROOT_AGE_BLOCKS` (300 blocks, roughly an hour) old, so a prover
may legitimately ground against the oldest accepted root: `block_timestamp`
can trail wall-clock time by up to that window, and the transaction lands on
chain later still. Treat time locks written against it as coarse-grained.

`block_hash` is a lossy repacking of the execution block hash (each 8-byte
limb reduced into a field element, see `pod2utils::b256_to_hash`) — suitable
as opaque entropy, not for byte-exact comparison with the L1 hash.

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
- [x] Statements:
  - [x] Equal
  - [x] NotEqual
  - [x] LtEq
  - [x] Lt
  - [x] Contains
  - [x] NotContains
  - [x] Sum
  - [x] Product
  - [x] Max
  - [x] Hash
  - [ ] PublicKey
  - [ ] SignedBy
  - [x] ContainerInsert
  - [x] ContainerUpdate
  - [x] ContainerDelete
  - [x] DictContains
  - [x] DictNotContains
  - [x] SetContains
  - [x] SetNotContains
  - [x] ArrayContains
  - [x] GtEq
  - [x] Gt
  - [x] DictInsert
  - [x] DictUpdate
  - [x] DictDelete
  - [x] SetInsert
  - [x] SetDelete
  - [x] ArrayUpdate
- [ ] Execution time type checking without panics
- [ ] operator+
- [ ] operator\*
- [x] dependent action
- [x] pexe.zip support (packaged by the `pexe` crate's CLI)
- [x] manifest support
- [ ] error pretty print
- [x] forbid Object::set after the object has been used in other operations
- [ ] read a field of an output created in the same action (`out.field`)

# Test example

The example in the test `test_sdk_1` produces the following podlang code:

```
use module 0xc2b96ca2c6970e4e950d09408011691c21b6c9c24610e74aec471ea53e0ace65 as tx
use intro Vdf(count, input, output) from 0xab82223f501b5056f458f063eb2fc073f8ac01f2ea178a3a2303394fec6828a0
use intro LtEqU256(lhs, rhs) from 0xe0595e5c75467e5a27bd30fa48a45e1dcc66a327076e5ce7c02ce33dfe357311

record StateHeader = (block_number, block_timestamp, block_hash, created, nullifiers, prior_state_history)
record FindLogIO = (out_log)
record FindLogInitials = (log)
record CraftWoodIO = (in_log, out_wood)
record CraftWoodInitials = (wood)
record CraftSticksIO = (in_wood, out_stick_a, out_stick_b)
record CraftSticksChain = (step_0, step_1)
record CraftSticksInitials = (stick_a, stick_b)
record CraftWoodPickIO = (in_wood, in_stick, out_pick)
record CraftWoodPickChain = (step_0, step_1)
record CraftWoodPickInitials = (pick)
record UseWoodPickIO = (in_wood_pick, out_wood_pick)
record MineStoneWithWoodPickIO = (out_stone)
record MineStoneWithWoodPickInitials = (stone)

// Actions

FindLog(io FindLogIO, state_header StateHeader, chain0, chain, private: log0, work, initials FindLogInitials) = AND(
  Vdf(3, log0, work)
  DictUpdate(log0, "work", work, initials.log)
  tx::TxInsert(chain0, chain, initials.log, io.out_log, @self_predicate(IsLog))
)

CraftWood(io CraftWoodIO, state_header StateHeader, chain0, chain, private: chain1, wood0, key, initials CraftWoodInitials) = AND(
  DictUpdate(wood0, "key", key, initials.wood)
  LtEqU256(initials.wood, Raw(0x0020000000000000000000000000000000000000000000000000000000000000))
  tx::TxDelete(chain0, chain1, io.in_log, @self_predicate(IsLog))
  tx::TxInsert(chain1, chain, initials.wood, io.out_wood, @self_predicate(IsWood))
)

CraftSticks(io CraftSticksIO, state_header StateHeader, chain0, chain, private: chain_steps CraftSticksChain, initials CraftSticksInitials) = AND(
  tx::TxDelete(chain0, chain_steps.step_0, io.in_wood, @self_predicate(IsWood))
  tx::TxInsert(chain_steps.step_0, chain_steps.step_1, initials.stick_a, io.out_stick_a, @self_predicate(IsStick))
  tx::TxInsert(chain_steps.step_1, chain, initials.stick_b, io.out_stick_b, @self_predicate(IsStick))
)

CraftWoodPick(io CraftWoodPickIO, state_header StateHeader, chain0, chain, private: chain_steps CraftWoodPickChain, initials CraftWoodPickInitials) = AND(
  DictContains(initials.pick, "durability", 100)
  tx::TxDelete(chain0, chain_steps.step_0, io.in_wood, @self_predicate(IsWood))
  tx::TxDelete(chain_steps.step_0, chain_steps.step_1, io.in_stick, @self_predicate(IsStick))
  tx::TxInsert(chain_steps.step_1, chain, initials.pick, io.out_pick, @self_predicate(IsWoodPick))
)

UseWoodPick(io UseWoodPickIO, state_header StateHeader, chain0, chain, private: wood_pick0, wood_pick1, wood_pick2, durability, key, work) = AND(
  ArrayContains(io, UseWoodPickIO::in_wood_pick, wood_pick0)
  Gt(wood_pick0.durability, 0)
  Sum(durability, 1, wood_pick0.durability)
  DictUpdate(wood_pick0, "durability", durability, wood_pick1)
  DictUpdate(wood_pick1, "key", key, wood_pick2)
  Vdf(10, wood_pick2, work)
  DictUpdate(wood_pick2, "work", work, io.out_wood_pick)
  tx::TxMutate(chain0, chain, wood_pick0, io.out_wood_pick, @self_predicate(IsWoodPick))
)

MineStoneWithWoodPick(io MineStoneWithWoodPickIO, state_header StateHeader, chain0, chain, private: chain1, _UseWoodPick_io_0 UseWoodPickIO, initials MineStoneWithWoodPickInitials) = AND(
  UseWoodPick(_UseWoodPick_io_0, state_header, chain0, chain1)
  tx::TxInsert(chain1, chain, initials.stone, io.out_stone, @self_predicate(IsStone))
)

// Bridges

IsLogFromFindLog(state, state_header, chain0, chain, private: io FindLogIO) = AND(
  ArrayContains(io, FindLogIO::out_log, state)
  FindLog(io, state_header, chain0, chain)
)

IsLogFromCraftWood(state, state_header, chain0, chain, private: io CraftWoodIO) = AND(
  ArrayContains(io, CraftWoodIO::in_log, state)
  CraftWood(io, state_header, chain0, chain)
)

IsWoodFromCraftWood(state, state_header, chain0, chain, private: io CraftWoodIO) = AND(
  ArrayContains(io, CraftWoodIO::out_wood, state)
  CraftWood(io, state_header, chain0, chain)
)

IsWoodFromCraftSticks(state, state_header, chain0, chain, private: io CraftSticksIO) = AND(
  ArrayContains(io, CraftSticksIO::in_wood, state)
  CraftSticks(io, state_header, chain0, chain)
)

IsStickFromCraftSticks_stick_a(state, state_header, chain0, chain, private: io CraftSticksIO) = AND(
  ArrayContains(io, CraftSticksIO::out_stick_a, state)
  CraftSticks(io, state_header, chain0, chain)
)

IsStickFromCraftSticks_stick_b(state, state_header, chain0, chain, private: io CraftSticksIO) = AND(
  ArrayContains(io, CraftSticksIO::out_stick_b, state)
  CraftSticks(io, state_header, chain0, chain)
)

IsWoodFromCraftWoodPick(state, state_header, chain0, chain, private: io CraftWoodPickIO) = AND(
  ArrayContains(io, CraftWoodPickIO::in_wood, state)
  CraftWoodPick(io, state_header, chain0, chain)
)

IsStickFromCraftWoodPick(state, state_header, chain0, chain, private: io CraftWoodPickIO) = AND(
  ArrayContains(io, CraftWoodPickIO::in_stick, state)
  CraftWoodPick(io, state_header, chain0, chain)
)

IsWoodPickFromCraftWoodPick(state, state_header, chain0, chain, private: io CraftWoodPickIO) = AND(
  ArrayContains(io, CraftWoodPickIO::out_pick, state)
  CraftWoodPick(io, state_header, chain0, chain)
)

IsWoodPickFromUseWoodPick(state, state_header, chain0, chain, private: io UseWoodPickIO) = AND(
  ArrayContains(io, UseWoodPickIO::out_wood_pick, state)
  UseWoodPick(io, state_header, chain0, chain)
)

IsStoneFromMineStoneWithWoodPick(state, state_header, chain0, chain, private: io MineStoneWithWoodPickIO) = AND(
  ArrayContains(io, MineStoneWithWoodPickIO::out_stone, state)
  MineStoneWithWoodPick(io, state_header, chain0, chain)
)

// Classes

IsLog(state, state_header StateHeader, chain0, chain) = OR(
  IsLogFromFindLog(state, state_header, chain0, chain)
  IsLogFromCraftWood(state, state_header, chain0, chain)
)

IsWood(state, state_header StateHeader, chain0, chain) = OR(
  IsWoodFromCraftWood(state, state_header, chain0, chain)
  IsWoodFromCraftSticks(state, state_header, chain0, chain)
  IsWoodFromCraftWoodPick(state, state_header, chain0, chain)
)

IsStick(state, state_header StateHeader, chain0, chain) = OR(
  IsStickFromCraftSticks_stick_a(state, state_header, chain0, chain)
  IsStickFromCraftSticks_stick_b(state, state_header, chain0, chain)
  IsStickFromCraftWoodPick(state, state_header, chain0, chain)
)

IsWoodPick(state, state_header StateHeader, chain0, chain) = OR(
  IsWoodPickFromCraftWoodPick(state, state_header, chain0, chain)
  IsWoodPickFromUseWoodPick(state, state_header, chain0, chain)
)

IsStone(state, state_header StateHeader, chain0, chain) = OR(
  IsStoneFromMineStoneWithWoodPick(state, state_header, chain0, chain)
)
```
