# nanoverse

A cut-down microverse, written against the SDK surface microverse predates. It covers the same spine (build a ship, claim and survey a sector, detect a signal, scan a body, extract from it, mint and reveal a coordinate, warp, refine and merge stacks) and exists to show, in one readable file, what each of those SDK features is for.

```
just pexe inspect predicates examples/nanoverse          # the podlang it renders to
just pexe inspect predicates --action RevealCoordinate examples/nanoverse
just pexe inspect classes examples/nanoverse             # per-class field shapes
just pexe inspect plan --action MoveShipX examples/nanoverse
just pexe build examples/nanoverse                       # compile and pack
```

## What each action demonstrates

| Feature | Where | What it replaces |
| --- | --- | --- |
| `st_equal` | `require_version`, and every action | `st_sum(x, 0, y)`. In microverse's compiled batch 26,666 of 29,617 `Sum` statements have a zero operand and are equality checks: 21,318 against a constant, 4,648 between two fields, 700 pinning a copy. `Sum` also type checks as an integer, so it cannot make the same claim about a `Raw` or a container. |
| `st_gt_eq`, `st_lt_eq` | `DetectSignal`, `UseCoordinate`, `ExtractResource`, `ExtractCoordinate` | `Gt(x, -1)` for `x >= 0`, 960 of microverse's 2,373 `Gt` statements. |
| Anchored field ref as a value | `ClaimSector`, `DetectSignal`, `ScanBody` | `unsafe { obj.f - 0 }` plus a `Sum` to pin the copy, which costs one statement and one private wildcard per field carried. A ref resolves against the object version live where it is used, so a read taken before a later `update()` already sees the pre-update value: freezing it into a copy first is unnecessary. |
| `+` and `*` on vars inside `unsafe` | `UpgradeShip`, `MoveShipX`, `MergeResources`, `ExtractResource` | `unsafe { x - (0 - 1) }`. |
| Reading `stable_identifier` off an anchored ref | `SurveySector*`, `DetectSignal`, `ScanBody` | `random()` + `var_assign` + `update("stable_identifier", ...)`, which microverse does at 22 sites across 40 action predicates. `var_assign` only sets a value at execution time and emits nothing, so on a mutated object the band or the provenance field ends up gating a wildcard the prover picks freely. |
| One `set()` after the inputs are declared | `ScanBody` | A placeholder in every field the output cannot fill yet, then an update per field to overwrite it. |
| Container literal plus `array_get` | `UpgradeShip`, `MoveShipX`, `DetectSignal`, `ScanBody`, `ExtractResource`, `RevealCoordinate` | One action per row. 2,017 of microverse's 2,070 actions are one-line rows of literals over 43 helpers. A table read is one `ArrayContains` and one private wildcard whatever the table's size. |
| `Raw` bounds as table or literal values | `SurveySector*`, and `top_limb_u256` in a row | Per-band helper variants (`*_ungated`, `*_lower`, `*_upper`, `*_range`). `top_limb_u256` returns a literal, so a band can live in a row alongside the rest of it. |
| `intro_vdf` with a var count | `UpgradeShip`, `ScanBody` | A separate predicate per difficulty. |
| Nested container in a row, stored as a field | `ScanBody` (`body.kinds` is a `Set`) | Nothing in microverse: it has no containers at all. |
| `st_set_contains` against an object field | `ExtractResource` | An action per allowed value. |
| `st_product` | `ExtractResource` | Baking every product into a row literal. microverse has no multiplication anywhere. |
| Dictionary read at a var name | `RefineResource`, `ScrapResource` | One action per recipe. A pod2 dictionary is keyed by the hash of a string, so a name reaches a row where an index cannot. microverse spends 324 actions on this family and cannot fold them as encoded: its selector is a sparse integer type code, which an array has no position for. |
| `st_dict_not_contains` | `ScrapResource` | An action per unrefinable kind. The absence of a row is one statement whatever the table holds. |
| `var` naming a literal | `RefineResource`, `ScrapResource` | Repeating the table expression at each use site. `var` on a literal declares no wildcard: it names the value for the script. |
| Reading a field of an output the action built | `RefineResource` | Keeping a second copy of the value in a wildcard so it can be checked after it is written. |
| `state_header` | `ClaimSector`, `MoveShipX` | Nothing in microverse: it never reads the header. `ClaimSector` records `block_number`; `MoveShipX` gates on `block_timestamp`, which describes the grounding root rather than the including block, so the lock it writes is coarse by construction. |
| `subaction`, and reading its object | `WarpToCoordinate` | A script helper, which cannot hand the parent a proof-bound field of the object it touched. |

Rendered, that is 16 actions, 48 predicates, 264 statements and an `Isship` OR of 8 branches, against microverse's 2,070 actions, 7,873 predicates, 85,973 statements and an `Isship` OR of 1,852.

## The rule that decides whether a family collapses

An action takes no scalar arguments: `Executor::action` receives objects and nothing else. So a table index has to be computable at execution time from an object field, a literal, or arithmetic on those.

- **Rows the data picks collapse.** Ship tier (`ship.tier`), the signals a sector can turn up (`sector.profile`), the body a signal resolves to (`signal.category`), the catalog row a coordinate carries (`coordinate.row`). The row is bound by an authenticated field, which is strictly better than binding it by which action the prover picked.
- **Rows the player picks do not.** `MoveShipX` stays one action per direction, because the action name is the only channel that choice has.
- **Selectors that are identities do not either.** `SurveySector*` bands `sector.stable_identifier` with `intro_lt_eq_u256`: a band picks a row without needing an index, and there is no arithmetic that turns a 256-bit identity into a table position.

`ExtractCoordinate` is where the third case gets handled without banding: it writes the body's own counter into the coordinate as `row`, so the destination a coordinate will reveal is fixed by its lineage at mint time, and `RevealCoordinate` is one action for the whole catalog. microverse mints coordinates with no selector at all and offers one `Reveal*` action per row, gated only on `revealed == 0` and a pool floor, so the prover picks the destination.

## What is deliberately not collapsed

- `UseCoordinate` spends a use but cannot delete the coordinate on the last one: whether an object is mutated or consumed is fixed where it is declared, so "reusable or final" is still two actions.
- `MergeResources` takes exactly two stacks. More arities need more actions until there is a bounded `input_many`.
- Per-kind resource pools would want `body.pools` as a dictionary read at a var key. `dict_get` handles the read, but `Object::update` takes a literal key and the container transition statements relate two containers without producing one, so there is no way to write back at a var key. `body.remaining` is a scalar here for that reason.

## Running it against synthetic fixtures

`pexe inspect plan` mints synthetic inputs from a class signature derived out of the compiled batch. Eleven of the sixteen actions do not plan, for two reasons, both of which microverse hits as well (`MovePositiveX`, `ClaimSector`, `SurveySector_01_Sparse` and `RevealWarpCoordinate001` all fail there for the same reasons). Every failure is a reported error; none of them panic.

- **One mint per field cannot satisfy every precondition.** `SurveySector*` needs `profile == 0` and the minter picks from the profiles a survey writes; `UseCoordinate` and `WarpToCoordinate` need `revealed == 1`. Inherent to synthetic minting.
- **A field only learns a value from a literal in the batch.** Fields written from a table row, from another object's field, or from a witness mint as a random `Raw`, so an action that then does arithmetic on one, indexes a table with one, or reads a container field fails at execution: `DetectSignal`, `ScanBody`, `ExtractResource`, `ExtractCoordinate`, `RevealCoordinate`, `RefineResource`, `MergeResources`. `inspect classes` shows which fields the analyzer resolved.

`BuildShip`, `UpgradeShip`, `MoveShipX`, `ClaimSector` and `ScrapResource` plan as they are. `MoveShipX` survives its cooldown because the synthetic header carries `block_timestamp = 1` while `ready_at` mints from the literal `BuildShip` writes.

## The podlang it renders to

Generated by `just pexe inspect predicates examples/nanoverse`, reproduced here so the file can be read against what it compiles to. Regenerate it after editing the plugin.

The imports, and the records the formatter coalesces each action's object slots into:

```
use module 0xc2b96ca2c6970e4e950d09408011691c21b6c9c24610e74aec471ea53e0ace65 as tx
use intro Vdf(count, input, output) from 0xab82223f501b5056f458f063eb2fc073f8ac01f2ea178a3a2303394fec6828a0
use intro LtEqU256(lhs, rhs) from 0xe0595e5c75467e5a27bd30fa48a45e1dcc66a327076e5ce7c02ce33dfe357311

record StateHeader = (block_number, block_timestamp, block_hash, created, nullifiers, prior_state_history)
record BuildShipIO = (out_ship)
record BuildShipInitials = (ship)
record UpgradeShipIO = (in_ship, out_ship)
record MoveShipXIO = (in_ship, out_ship)
record ClaimSectorIO = (in_ship, out_ship, out_sector)
record ClaimSectorInitials = (sector)
record SurveySectorSparseIO = (in_sector, out_sector)
record SurveySectorRichIO = (in_sector, out_sector)
record DetectSignalIO = (in_ship, in_sector, out_ship, out_sector, out_signal)
record DetectSignalChain = (step_0, step_1)
record DetectSignalInitials = (signal)
record ScanBodyIO = (in_ship, in_signal, out_ship, out_body)
record ScanBodyChain = (step_0, step_1)
record ScanBodyInitials = (body)
record ExtractResourceIO = (in_ship, in_body, out_ship, out_body, out_resource)
record ExtractResourceChain = (step_0, step_1)
record ExtractResourceInitials = (resource)
record ExtractCoordinateIO = (in_body, out_body, out_coordinate)
record ExtractCoordinateInitials = (coordinate)
record RevealCoordinateIO = (in_coordinate, out_coordinate)
record UseCoordinateIO = (in_coordinate, out_coordinate)
record WarpToCoordinateIO = (in_ship, out_ship)
record RefineResourceIO = (in_source, out_refined)
record RefineResourceInitials = (refined)
record ScrapResourceIO = (in_source)
record MergeResourcesIO = (in_destination, in_source, out_destination)
```

The actions. Every pattern in the table above is visible here: `Equal` where microverse would write `Sum(x, 0, y)`, `GtEq` where it would write `Gt(x, -1)`, whole tables embedded as one `ArrayContains` or `DictContains` argument, `Raw(0x...)` bounds, `row.field` and `ship0.x` as anchored refs standing where a copied wildcard would otherwise go, and `Vdf(row.vdf, ...)` taking its iteration count out of a row.

```
BuildShip(io BuildShipIO, state_header StateHeader, chain0, chain, private: work, initials BuildShipInitials) = AND(
  DictContains(initials.ship, "v", 1)
  DictContains(initials.ship, "tier", 0)
  DictContains(initials.ship, "x", 0)
  DictContains(initials.ship, "y", 0)
  DictContains(initials.ship, "z", 0)
  DictContains(initials.ship, "ready_at", 0)
  Vdf(3, initials.ship, work)
  tx::TxInsert(chain0, chain, initials.ship, io.out_ship, @self_predicate(Isship))
)

UpgradeShip(io UpgradeShipIO, state_header StateHeader, chain0, chain, private: ship0, ship1, next_tier, row, work, _rand3) = AND(
  ArrayContains(io, UpgradeShipIO::in_ship, ship0)
  Equal(ship0.v, 1)
  Sum(ship0.tier, 1, next_tier)
  ArrayContains([{"extract_base": 10, "vdf": 3, "step": 1}, {"extract_base": 50, "vdf": 6, "step": 10}, {"extract_base": 250, "vdf": 9, "step": 100}], next_tier, row)
  DictUpdate(ship0, "tier", next_tier, ship1)
  Vdf(row.vdf, ship1, work)
  DictUpdate(ship1, "key", _rand3, io.out_ship)
  tx::TxMutate(chain0, chain, ship0, io.out_ship, @self_predicate(Isship))
)

MoveShipX(io MoveShipXIO, state_header StateHeader, chain0, chain, private: ship0, ship1, ship2, row, next_x, _rand2) = AND(
  ArrayContains(io, MoveShipXIO::in_ship, ship0)
  Equal(ship0.v, 1)
  Gt(state_header.block_timestamp, ship0.ready_at)
  ArrayContains([{"extract_base": 10, "vdf": 3, "step": 1}, {"extract_base": 50, "vdf": 6, "step": 10}, {"extract_base": 250, "vdf": 9, "step": 100}], ship0.tier, row)
  Sum(ship0.x, row.step, next_x)
  DictUpdate(ship0, "x", next_x, ship1)
  DictUpdate(ship1, "ready_at", state_header.block_timestamp, ship2)
  DictUpdate(ship2, "key", _rand2, io.out_ship)
  tx::TxMutate(chain0, chain, ship0, io.out_ship, @self_predicate(Isship))
)

ClaimSector(io ClaimSectorIO, state_header StateHeader, chain0, chain, private: chain1, ship0, _rand0, initials ClaimSectorInitials) = AND(
  ArrayContains(io, ClaimSectorIO::in_ship, ship0)
  Equal(ship0.v, 1)
  DictContains(initials.sector, "v", 1)
  DictContains(initials.sector, "x", ship0.x)
  DictContains(initials.sector, "y", ship0.y)
  DictContains(initials.sector, "z", ship0.z)
  DictContains(initials.sector, "profile", 0)
  DictContains(initials.sector, "slots_left", 0)
  DictContains(initials.sector, "next_slot", 0)
  DictContains(initials.sector, "claimed_at", state_header.block_number)
  DictUpdate(ship0, "key", _rand0, io.out_ship)
  tx::TxMutate(chain0, chain1, ship0, io.out_ship, @self_predicate(Isship))
  tx::TxInsert(chain1, chain, initials.sector, io.out_sector, @self_predicate(Issector))
)

SurveySectorSparse(io SurveySectorSparseIO, state_header StateHeader, chain0, chain, private: sector0, sector1, sector2, _rand0) = AND(
  ArrayContains(io, SurveySectorSparseIO::in_sector, sector0)
  LtEqU256(sector0.stable_identifier, Raw(0x5555555555555555000000000000000000000000000000000000000000000000))
  Equal(sector0.v, 1)
  Equal(sector0.profile, 0)
  DictUpdate(sector0, "profile", 1, sector1)
  DictUpdate(sector1, "slots_left", 3, sector2)
  DictUpdate(sector2, "key", _rand0, io.out_sector)
  tx::TxMutate(chain0, chain, sector0, io.out_sector, @self_predicate(Issector))
)

SurveySectorRich(io SurveySectorRichIO, state_header StateHeader, chain0, chain, private: sector0, sector1, sector2, _rand0) = AND(
  ArrayContains(io, SurveySectorRichIO::in_sector, sector0)
  LtEqU256(Raw(0x5555555555555556000000000000000000000000000000000000000000000000), sector0.stable_identifier)
  Equal(sector0.v, 1)
  Equal(sector0.profile, 0)
  DictUpdate(sector0, "profile", 2, sector1)
  DictUpdate(sector1, "slots_left", 5, sector2)
  DictUpdate(sector2, "key", _rand0, io.out_sector)
  tx::TxMutate(chain0, chain, sector0, io.out_sector, @self_predicate(Issector))
)

DetectSignal(io DetectSignalIO, state_header StateHeader, chain0, chain, private: ship0, sector0, sector1, sector2, row, left, next_slot, _rand3, _rand4, chain_steps DetectSignalChain, initials DetectSignalInitials) = AND(
  ArrayContains(io, DetectSignalIO::in_ship, ship0)
  ArrayContains(io, DetectSignalIO::in_sector, sector0)
  Equal(ship0.v, 1)
  Equal(sector0.v, 1)
  Equal(ship0.x, sector0.x)
  Equal(ship0.y, sector0.y)
  Equal(ship0.z, sector0.z)
  ArrayContains([{"slots": 1, "category": 0}, {"slots": 3, "category": 1}, {"slots": 5, "category": 2}], sector0.profile, row)
  Sum(left, 1, sector0.slots_left)
  GtEq(left, 0)
  Sum(sector0.next_slot, 1, next_slot)
  DictContains(initials.signal, "v", 1)
  DictContains(initials.signal, "category", row.category)
  DictContains(initials.signal, "origin", sector0.stable_identifier)
  DictContains(initials.signal, "slot", sector0.next_slot)
  DictUpdate(sector0, "slots_left", left, sector1)
  DictUpdate(sector1, "next_slot", next_slot, sector2)
  DictUpdate(sector2, "key", _rand3, io.out_sector)
  DictUpdate(ship0, "key", _rand4, io.out_ship)
  tx::TxMutate(chain0, chain_steps.step_0, ship0, io.out_ship, @self_predicate(Isship))
  tx::TxMutate(chain_steps.step_0, chain_steps.step_1, sector0, io.out_sector, @self_predicate(Issector))
  tx::TxInsert(chain_steps.step_1, chain, initials.signal, io.out_signal, @self_predicate(Issignal))
)

ScanBody(io ScanBodyIO, state_header StateHeader, chain0, chain, private: ship0, signal, row, work, _rand2, chain_steps ScanBodyChain, initials ScanBodyInitials) = AND(
  ArrayContains(io, ScanBodyIO::in_ship, ship0)
  ArrayContains(io, ScanBodyIO::in_signal, signal)
  Equal(ship0.v, 1)
  Equal(signal.v, 1)
  ArrayContains([{"body_type": 1, "kinds": #["ore", "slag"], "vdf": 4, "pool": 500, "richness": 1, "primary_kind": "ore"}, {"body_type": 2, "kinds": #["shard", "crystal", "dust"], "vdf": 8, "pool": 2000, "richness": 3, "primary_kind": "crystal"}, {"body_type": 3, "kinds": #["ice", "vapor"], "vdf": 12, "pool": 9000, "richness": 7, "primary_kind": "ice"}], signal.category, row)
  DictContains(initials.body, "v", 1)
  DictContains(initials.body, "body_type", row.body_type)
  DictContains(initials.body, "richness", row.richness)
  DictContains(initials.body, "primary_kind", row.primary_kind)
  DictContains(initials.body, "kinds", row.kinds)
  DictContains(initials.body, "remaining", row.pool)
  DictContains(initials.body, "origin", signal.stable_identifier)
  DictContains(initials.body, "slot", signal.slot)
  DictContains(initials.body, "next_coordinate", 0)
  Vdf(row.vdf, initials.body, work)
  DictUpdate(ship0, "key", _rand2, io.out_ship)
  tx::TxMutate(chain0, chain_steps.step_0, ship0, io.out_ship, @self_predicate(Isship))
  tx::TxDelete(chain_steps.step_0, chain_steps.step_1, signal, @self_predicate(Issignal))
  tx::TxInsert(chain_steps.step_1, chain, initials.body, io.out_body, @self_predicate(Isbody))
)

ExtractResource(io ExtractResourceIO, state_header StateHeader, chain0, chain, private: ship0, body0, body1, tier, amount, left, _rand3, _rand4, chain_steps ExtractResourceChain, initials ExtractResourceInitials) = AND(
  ArrayContains(io, ExtractResourceIO::in_ship, ship0)
  ArrayContains(io, ExtractResourceIO::in_body, body0)
  Equal(ship0.v, 1)
  Equal(body0.v, 1)
  ArrayContains([{"extract_base": 10, "vdf": 3, "step": 1}, {"extract_base": 50, "vdf": 6, "step": 10}, {"extract_base": 250, "vdf": 9, "step": 100}], ship0.tier, tier)
  Product(tier.extract_base, body0.richness, amount)
  Sum(left, amount, body0.remaining)
  GtEq(left, 0)
  SetContains(body0.kinds, body0.primary_kind)
  DictContains(initials.resource, "v", 1)
  DictContains(initials.resource, "kind", body0.primary_kind)
  DictContains(initials.resource, "amount", amount)
  DictUpdate(body0, "remaining", left, body1)
  DictUpdate(body1, "key", _rand3, io.out_body)
  DictUpdate(ship0, "key", _rand4, io.out_ship)
  tx::TxMutate(chain0, chain_steps.step_0, ship0, io.out_ship, @self_predicate(Isship))
  tx::TxMutate(chain_steps.step_0, chain_steps.step_1, body0, io.out_body, @self_predicate(Isbody))
  tx::TxInsert(chain_steps.step_1, chain, initials.resource, io.out_resource, @self_predicate(Isresource))
)

ExtractCoordinate(io ExtractCoordinateIO, state_header StateHeader, chain0, chain, private: chain1, body0, body1, next, _rand1, initials ExtractCoordinateInitials) = AND(
  ArrayContains(io, ExtractCoordinateIO::in_body, body0)
  Equal(body0.v, 1)
  Sum(body0.next_coordinate, 1, next)
  LtEq(next, 3)
  DictContains(initials.coordinate, "v", 1)
  DictContains(initials.coordinate, "row", body0.next_coordinate)
  DictContains(initials.coordinate, "source_pool", body0.remaining)
  DictContains(initials.coordinate, "revealed", 0)
  DictContains(initials.coordinate, "code", 0)
  DictContains(initials.coordinate, "x", 0)
  DictContains(initials.coordinate, "y", 0)
  DictContains(initials.coordinate, "z", 0)
  DictContains(initials.coordinate, "uses_left", 0)
  DictUpdate(body0, "next_coordinate", next, body1)
  DictUpdate(body1, "key", _rand1, io.out_body)
  tx::TxMutate(chain0, chain1, body0, io.out_body, @self_predicate(Isbody))
  tx::TxInsert(chain1, chain, initials.coordinate, io.out_coordinate, @self_predicate(Iscoordinate))
)

RevealCoordinate(io RevealCoordinateIO, state_header StateHeader, chain0, chain, private: coordinate0, coordinate1, coordinate2, coordinate3, coordinate4, coordinate5, coordinate6, row, _rand1) = AND(
  ArrayContains(io, RevealCoordinateIO::in_coordinate, coordinate0)
  Equal(coordinate0.v, 1)
  Equal(coordinate0.revealed, 0)
  ArrayContains([{"z": 42019687806, "uses": 3, "y": 968149119310, "x": 793814733, "floor": 50, "code": 101}, {"z": 3102, "uses": 2, "y": 605502947, "x": 327853439873, "floor": 100, "code": 102}, {"z": 86018776812, "uses": 1, "y": 465107091, "x": 105, "floor": 250, "code": 103}], coordinate0.row, row)
  Gt(coordinate0.source_pool, row.floor)
  DictUpdate(coordinate0, "revealed", 1, coordinate1)
  DictUpdate(coordinate1, "code", row.code, coordinate2)
  DictUpdate(coordinate2, "x", row.x, coordinate3)
  DictUpdate(coordinate3, "y", row.y, coordinate4)
  DictUpdate(coordinate4, "z", row.z, coordinate5)
  DictUpdate(coordinate5, "uses_left", row.uses, coordinate6)
  DictUpdate(coordinate6, "key", _rand1, io.out_coordinate)
  tx::TxMutate(chain0, chain, coordinate0, io.out_coordinate, @self_predicate(Iscoordinate))
)

UseCoordinate(io UseCoordinateIO, state_header StateHeader, chain0, chain, private: coordinate0, coordinate1, left, _rand1) = AND(
  ArrayContains(io, UseCoordinateIO::in_coordinate, coordinate0)
  Equal(coordinate0.v, 1)
  Equal(coordinate0.revealed, 1)
  Sum(left, 1, coordinate0.uses_left)
  GtEq(left, 0)
  DictUpdate(coordinate0, "uses_left", left, coordinate1)
  DictUpdate(coordinate1, "key", _rand1, io.out_coordinate)
  tx::TxMutate(chain0, chain, coordinate0, io.out_coordinate, @self_predicate(Iscoordinate))
)

WarpToCoordinate(io WarpToCoordinateIO, state_header StateHeader, chain0, chain, private: chain1, coordinate, ship0, ship1, ship2, ship3, _rand0, _UseCoordinate_io_0 UseCoordinateIO) = AND(
  ArrayContains(io, WarpToCoordinateIO::in_ship, ship0)
  ArrayContains(_UseCoordinate_io_0, UseCoordinateIO::out_coordinate, coordinate)
  UseCoordinate(_UseCoordinate_io_0, state_header, chain0, chain1)
  Equal(ship0.v, 1)
  DictUpdate(ship0, "x", coordinate.x, ship1)
  DictUpdate(ship1, "y", coordinate.y, ship2)
  DictUpdate(ship2, "z", coordinate.z, ship3)
  DictUpdate(ship3, "key", _rand0, io.out_ship)
  tx::TxMutate(chain1, chain, ship0, io.out_ship, @self_predicate(Isship))
)

RefineResource(io RefineResourceIO, state_header StateHeader, chain0, chain, private: chain1, source, refined0, row, amount, initials RefineResourceInitials) = AND(
  ArrayContains(io, RefineResourceIO::in_source, source)
  ArrayContains(initials, RefineResourceInitials::refined, refined0)
  Equal(source.v, 1)
  DictContains({"ice": {"factor": 3, "out": "water"}, "ore": {"factor": 2, "out": "ingot"}, "crystal": {"factor": 1, "out": "lens"}}, source.kind, row)
  Product(source.amount, row.factor, amount)
  DictContains(refined0, "v", 1)
  DictContains(refined0, "kind", row.out)
  DictContains(refined0, "amount", amount)
  Gt(refined0.amount, 0)
  tx::TxDelete(chain0, chain1, source, @self_predicate(Isresource))
  tx::TxInsert(chain1, chain, refined0, io.out_refined, @self_predicate(Isresource))
)

ScrapResource(io ScrapResourceIO, state_header StateHeader, chain0, chain, private: source) = AND(
  ArrayContains(io, ScrapResourceIO::in_source, source)
  Equal(source.v, 1)
  DictNotContains({"ice": {"factor": 3, "out": "water"}, "ore": {"factor": 2, "out": "ingot"}, "crystal": {"factor": 1, "out": "lens"}}, source.kind)
  tx::TxDelete(chain0, chain, source, @self_predicate(Isresource))
)

MergeResources(io MergeResourcesIO, state_header StateHeader, chain0, chain, private: chain1, destination0, destination1, source, total, _rand1) = AND(
  ArrayContains(io, MergeResourcesIO::in_destination, destination0)
  ArrayContains(io, MergeResourcesIO::in_source, source)
  Equal(destination0.v, 1)
  Equal(source.v, 1)
  Equal(source.kind, destination0.kind)
  GtEq(source.amount, 0)
  Sum(destination0.amount, source.amount, total)
  DictUpdate(destination0, "amount", total, destination1)
  DictUpdate(destination1, "key", _rand1, io.out_destination)
  tx::TxMutate(chain0, chain1, destination0, io.out_destination, @self_predicate(Isresource))
  tx::TxDelete(chain1, chain, source, @self_predicate(Isresource))
)
```

One bridge per object slot per action, and the `IsX` OR over them. This is the part that scales with the action count rather than with what the actions say: `Isship` is 8 branches here and 1,852 in microverse, because every row of a table there is its own action.

```
IsshipFromBuildShip(state, state_header, chain0, chain, private: io BuildShipIO) = AND(
  ArrayContains(io, BuildShipIO::out_ship, state)
  BuildShip(io, state_header, chain0, chain)
)

IsshipFromUpgradeShip(state, state_header, chain0, chain, private: io UpgradeShipIO) = AND(
  ArrayContains(io, UpgradeShipIO::out_ship, state)
  UpgradeShip(io, state_header, chain0, chain)
)

IsshipFromMoveShipX(state, state_header, chain0, chain, private: io MoveShipXIO) = AND(
  ArrayContains(io, MoveShipXIO::out_ship, state)
  MoveShipX(io, state_header, chain0, chain)
)

IsshipFromClaimSector(state, state_header, chain0, chain, private: io ClaimSectorIO) = AND(
  ArrayContains(io, ClaimSectorIO::out_ship, state)
  ClaimSector(io, state_header, chain0, chain)
)

IssectorFromClaimSector(state, state_header, chain0, chain, private: io ClaimSectorIO) = AND(
  ArrayContains(io, ClaimSectorIO::out_sector, state)
  ClaimSector(io, state_header, chain0, chain)
)

IssectorFromSurveySectorSparse(state, state_header, chain0, chain, private: io SurveySectorSparseIO) = AND(
  ArrayContains(io, SurveySectorSparseIO::out_sector, state)
  SurveySectorSparse(io, state_header, chain0, chain)
)

IssectorFromSurveySectorRich(state, state_header, chain0, chain, private: io SurveySectorRichIO) = AND(
  ArrayContains(io, SurveySectorRichIO::out_sector, state)
  SurveySectorRich(io, state_header, chain0, chain)
)

IsshipFromDetectSignal(state, state_header, chain0, chain, private: io DetectSignalIO) = AND(
  ArrayContains(io, DetectSignalIO::out_ship, state)
  DetectSignal(io, state_header, chain0, chain)
)

IssectorFromDetectSignal(state, state_header, chain0, chain, private: io DetectSignalIO) = AND(
  ArrayContains(io, DetectSignalIO::out_sector, state)
  DetectSignal(io, state_header, chain0, chain)
)

IssignalFromDetectSignal(state, state_header, chain0, chain, private: io DetectSignalIO) = AND(
  ArrayContains(io, DetectSignalIO::out_signal, state)
  DetectSignal(io, state_header, chain0, chain)
)

IsshipFromScanBody(state, state_header, chain0, chain, private: io ScanBodyIO) = AND(
  ArrayContains(io, ScanBodyIO::out_ship, state)
  ScanBody(io, state_header, chain0, chain)
)

IssignalFromScanBody(state, state_header, chain0, chain, private: io ScanBodyIO) = AND(
  ArrayContains(io, ScanBodyIO::in_signal, state)
  ScanBody(io, state_header, chain0, chain)
)

IsbodyFromScanBody(state, state_header, chain0, chain, private: io ScanBodyIO) = AND(
  ArrayContains(io, ScanBodyIO::out_body, state)
  ScanBody(io, state_header, chain0, chain)
)

IsshipFromExtractResource(state, state_header, chain0, chain, private: io ExtractResourceIO) = AND(
  ArrayContains(io, ExtractResourceIO::out_ship, state)
  ExtractResource(io, state_header, chain0, chain)
)

IsbodyFromExtractResource(state, state_header, chain0, chain, private: io ExtractResourceIO) = AND(
  ArrayContains(io, ExtractResourceIO::out_body, state)
  ExtractResource(io, state_header, chain0, chain)
)

IsresourceFromExtractResource(state, state_header, chain0, chain, private: io ExtractResourceIO) = AND(
  ArrayContains(io, ExtractResourceIO::out_resource, state)
  ExtractResource(io, state_header, chain0, chain)
)

IsbodyFromExtractCoordinate(state, state_header, chain0, chain, private: io ExtractCoordinateIO) = AND(
  ArrayContains(io, ExtractCoordinateIO::out_body, state)
  ExtractCoordinate(io, state_header, chain0, chain)
)

IscoordinateFromExtractCoordinate(state, state_header, chain0, chain, private: io ExtractCoordinateIO) = AND(
  ArrayContains(io, ExtractCoordinateIO::out_coordinate, state)
  ExtractCoordinate(io, state_header, chain0, chain)
)

IscoordinateFromRevealCoordinate(state, state_header, chain0, chain, private: io RevealCoordinateIO) = AND(
  ArrayContains(io, RevealCoordinateIO::out_coordinate, state)
  RevealCoordinate(io, state_header, chain0, chain)
)

IscoordinateFromUseCoordinate(state, state_header, chain0, chain, private: io UseCoordinateIO) = AND(
  ArrayContains(io, UseCoordinateIO::out_coordinate, state)
  UseCoordinate(io, state_header, chain0, chain)
)

IsshipFromWarpToCoordinate(state, state_header, chain0, chain, private: io WarpToCoordinateIO) = AND(
  ArrayContains(io, WarpToCoordinateIO::out_ship, state)
  WarpToCoordinate(io, state_header, chain0, chain)
)

IsresourceFromRefineResource_source(state, state_header, chain0, chain, private: io RefineResourceIO) = AND(
  ArrayContains(io, RefineResourceIO::in_source, state)
  RefineResource(io, state_header, chain0, chain)
)

IsresourceFromRefineResource_refined(state, state_header, chain0, chain, private: io RefineResourceIO) = AND(
  ArrayContains(io, RefineResourceIO::out_refined, state)
  RefineResource(io, state_header, chain0, chain)
)

IsresourceFromScrapResource(state, state_header, chain0, chain, private: io ScrapResourceIO) = AND(
  ArrayContains(io, ScrapResourceIO::in_source, state)
  ScrapResource(io, state_header, chain0, chain)
)

IsresourceFromMergeResources_destination(state, state_header, chain0, chain, private: io MergeResourcesIO) = AND(
  ArrayContains(io, MergeResourcesIO::out_destination, state)
  MergeResources(io, state_header, chain0, chain)
)

IsresourceFromMergeResources_source(state, state_header, chain0, chain, private: io MergeResourcesIO) = AND(
  ArrayContains(io, MergeResourcesIO::in_source, state)
  MergeResources(io, state_header, chain0, chain)
)

Isship(state, state_header StateHeader, chain0, chain) = OR(
  IsshipFromBuildShip(state, state_header, chain0, chain)
  IsshipFromUpgradeShip(state, state_header, chain0, chain)
  IsshipFromMoveShipX(state, state_header, chain0, chain)
  IsshipFromClaimSector(state, state_header, chain0, chain)
  IsshipFromDetectSignal(state, state_header, chain0, chain)
  IsshipFromScanBody(state, state_header, chain0, chain)
  IsshipFromExtractResource(state, state_header, chain0, chain)
  IsshipFromWarpToCoordinate(state, state_header, chain0, chain)
)

Issector(state, state_header StateHeader, chain0, chain) = OR(
  IssectorFromClaimSector(state, state_header, chain0, chain)
  IssectorFromSurveySectorSparse(state, state_header, chain0, chain)
  IssectorFromSurveySectorRich(state, state_header, chain0, chain)
  IssectorFromDetectSignal(state, state_header, chain0, chain)
)

Issignal(state, state_header StateHeader, chain0, chain) = OR(
  IssignalFromDetectSignal(state, state_header, chain0, chain)
  IssignalFromScanBody(state, state_header, chain0, chain)
)

Isbody(state, state_header StateHeader, chain0, chain) = OR(
  IsbodyFromScanBody(state, state_header, chain0, chain)
  IsbodyFromExtractResource(state, state_header, chain0, chain)
  IsbodyFromExtractCoordinate(state, state_header, chain0, chain)
)

Isresource(state, state_header StateHeader, chain0, chain) = OR(
  IsresourceFromExtractResource(state, state_header, chain0, chain)
  IsresourceFromRefineResource_source(state, state_header, chain0, chain)
  IsresourceFromRefineResource_refined(state, state_header, chain0, chain)
  IsresourceFromScrapResource(state, state_header, chain0, chain)
  IsresourceFromMergeResources_destination(state, state_header, chain0, chain)
  IsresourceFromMergeResources_source(state, state_header, chain0, chain)
)

Iscoordinate(state, state_header StateHeader, chain0, chain) = OR(
  IscoordinateFromExtractCoordinate(state, state_header, chain0, chain)
  IscoordinateFromRevealCoordinate(state, state_header, chain0, chain)
  IscoordinateFromUseCoordinate(state, state_header, chain0, chain)
)
```
