# bitcraft — game rules

bitcraft is a crafting game where every item you own is private. The world doesn't see your inventory; the chain only sees that *something valid* happened. You play by mining raw resources, refining them through several tiers, and combining them into a finished rocket.

## The tech tree at a glance

```
  raw            t1                  t2                          t3                      t4 / t5
  ───            ──                  ──                          ──                      ───────
  Iron      ──► Ingot           ──► Steel ──► Gear           ──► Bearing                     Engine
  Copper    ──► Plate           ──► Wire / Coil              ──► Circuit                     Casing     ──► Rocket
  Oil       ──► Tar/Fuel/Gas    ──► Wax / Grease / Rubber    ──► Pistons                     Payload
  Sulfur    ──► Acid            ──► Solvent / Extract        ──► Resin / Panel
  Water     ──► (combines into recipes above)
  Cane      ──► Pulp            ──► Board                    ──► Panel
  Hemp      ──► Fiber           ──► Cloth                    ──► Canvas

   ┌─ branches the columns above flatten ────────────────────────────────────────────┐
   │ Steel and Gear are both T2 (Gear: 2 Steel → 3 Gear). Both feed T3:               │
   │   Steel + Grease → Bearing, Steel + Mold → Gear-via-blast-furnace variant.       │
   │ Acid splits at T2: Solvent (→ Resin via Grease+Rubber) and Extract (→ Panel,     │
   │ also yields Sludge → Catalyst / Binder).                                         │
   │ Gas (T1 byproduct of refining Oil) feeds Rubber (T2), which is required for      │
   │ Resin (T3).                                                                      │
   └──────────────────────────────────────────────────────────────────────────────────┘

   ┌─ gates ──────────────────────────────────────────────────────────────────┐
   │ MachineI unlocks t3 recipes; MachineII unlocks t4 + t5.                  │
   │ Stations (BlastFurnace, CircuitFab, CrackingUnit, ReactionChamber)       │
   │ unlock alternative recipe variants with better yield or faster timing.   │
   └──────────────────────────────────────────────────────────────────────────┘
```

## Actions, by tier

Every recipe is one command. If a class can be produced multiple ways, the command prompts you to pick a recipe variant.

### Raw resources

| Command | Proof | Produces |
|---|---|---|
| `mine-iron` | PoW | 1 Iron |
| `mine-copper` | PoW | 1 Copper |
| `mine-oil` | PoW | 1 Oil |
| `mine-sulfur` | VDF | 1 Sulfur |
| `farm-water` | VDF | 1 Water |
| `farm-cane` | VDF | 1 Cane |
| `farm-hemp` | VDF | 1 Hemp |

### T1 — refinement

| Command | Consumes | Produces |
|---|---|---|
| `craft-ingot` | 1 Iron (+ optional 1 Flux or DrillBit use) | 1–2 Ingot |
| `craft-plate` | 1 Copper | 1 Plate |
| `craft-pulp` | 1 Cane | 3 Pulp |
| `craft-fiber` | 1 Hemp | 3 Fiber |
| `craft-acid` | 1 Sulfur + 1 Water | 1–3 Acid (variant) |
| `craft-tar`, `craft-fuel`, `craft-gas` | 1 Oil + 1 Water (refinery recipes) | mixed byproducts |
| `craft-flux` | 1 Slag + 1 Water | 1 Flux |

### T2 — intermediates & tools

| Command | Consumes | Produces |
|---|---|---|
| `craft-steel` | 3 Ingot | 2 Steel (`-blast` variant uses BlastFurnace, faster, yields Slag) |
| `craft-wire` | 1 Plate | 3 Wire |
| `craft-cloth` | 2 Fiber | 1 Cloth |
| `craft-board` | 1 Pulp + 1 Water | 1 Board (+ 1 Lye byproduct) |
| `craft-wax`, `craft-grease` | 1 Tar | distillates |
| `craft-solvent` | 1 Fuel + 1 Acid (or +1 Lye) | 1 Solvent |
| `craft-coating` | 1 Fuel + 1 Wax | 1 Coating |
| `craft-rubber` | 3 Gas | 1 Rubber (Flash/Crude variants) |
| `craft-extract` | 2 Acid | 1 Extract + 1 Sludge |
| `craft-catalyst` | 3 Sludge + 1 Wire | 1 Catalyst |
| `craft-binder` | 3 Sludge + 1 Solvent | 1 Binder |
| `craft-gear` | 2 Steel | 3 Gear (`-cast` variant uses BlastFurnace + Mold for 5) |
| `craft-coil` | 3 Wire | 1 Coil |
| `craft-mold` | 1 Slag + 1 Tar | 1 Mold |
| `craft-drill-bit` | 1 Iron + 1 Gear | 1 DrillBit (durability tool) |
| `craft-soldering-iron` | 1 Wire + 1 Acid | 1 SolderingIron (durability tool) |
| `craft-pressure-valve` | 1 Oil + 1 Gear | 1 PressureValve (durability tool) |

### T3 — assemblies (need MachineI)

| Command | Consumes | Produces |
|---|---|---|
| `craft-bearing` | 1 Steel + 2 Grease | 2 Bearing |
| `craft-circuit` | 2 Wire + 1 Steel | 1 Circuit (Soldered/Fabbed/Flash/Crude variants) |
| `craft-canvas` | 2 Cloth + 1 Fiber + 1 Wax | 1 Canvas (variants) |
| `craft-panel` | 1 Board + 1 Extract | 2 Panel |
| `craft-pistons` | bearings + Coil + Grease | 1 Pistons |
| `craft-resin` | Grease + Solvent + Rubber | 1 Resin (Stable/Pressurized/Flash/Crude variants) |

### T4 + T5 — finals (need MachineII)

| Command | Consumes | Produces |
|---|---|---|
| `craft-engine` | Pistons + Gear + Circuit + Canvas | 1 Engine (`-tuned` variant via ReactionChamber + Catalyst) |
| `craft-casing` | Steel + Canvas + Bearing + Coil + Wire | 1 Casing (`-coated` uses Coating) |
| `craft-payload` | Panel + Circuit + Canvas + Wire + Grease | 1 Payload |
| `craft-rocket` | Engine + Casing + Payload + 2 Resin | **1 Rocket** (win) |

### Gates — machines & stations

Build once, keep forever. Machines and stations are required to even *attempt* the recipes they gate (the action mutates them to prove possession).

| Command | Requires | Unlocks |
|---|---|---|
| `craft-machine-i` | inputs from t2 | T3 recipes + station builds |
| `craft-machine-ii` | MachineI + inputs | T4 + T5 recipes |
| `craft-blast-furnace` | MachineI | `*-blast` and `*-cast` variants |
| `craft-circuit-fab` | MachineI | `craft-circuit` fabbed variant |
| `craft-cracking-unit` | MachineI | Refinery `-cracked` variant |
| `craft-reaction-chamber` | MachineII | `-stable`/`-tuned` variants for extract/resin/engine/rocket |

## Recipe variants

A class with multiple recipes (e.g. `craft-circuit` has 5) prompts you to pick. The variants exist for trade-offs:

- **base** — vanilla recipe with the proof mix the recipe was designed around (PoW + VDF).
- **`-flash`** — PoW only, faster mean but variance up. Gamble for throughput.
- **`-crude`** — partial-recovery variant, slower but doesn't fully consume the inputs.
- **`-cracked`** / **`-soldered`** / **`-fabbed`** / **`-blast`** / **`-cast`** / **`-stable`** / **`-tuned`** / **`-pressurized`** — station- or tool-gated variants with better yield or deterministic timing.
- **`-flux`** / **`-drilled`** — consume an extra reagent or a durability charge for double yield.

## Tools (durability)

DrillBit, SolderingIron, and PressureValve are *durability tools* — each carries a `durability` field that decrements every time a recipe references them (via the `-drilled`, `-soldered`, `-pressurized` variants). When durability hits zero the tool is consumed.

## Why proofs

Every action produces a small zero-knowledge proof attached to the new object. The proof says "this object was reached by a valid sequence of actions" without revealing what those actions were or which objects were consumed. The chain stores the proof's commitment; observers can't tell what you have, only that your moves were legal.

A nullifier is published each time you consume an object. It prevents double-spending (no replaying the same Steel into two different circuits), but doesn't reveal which object was consumed.

## How to play

A reasonable first path:

1. `mine-iron`, `mine-copper`, `farm-water` — get a stockpile of raw materials.
2. `craft-ingot`, `craft-plate` — refine into T1.
3. `craft-steel`, `craft-wire`, `craft-gear` — climb to T2.
4. `craft-machine-i` — unlock T3.
5. `craft-bearing`, `craft-circuit`, … — build up T3 parts.
6. `craft-machine-ii` — unlock T4/T5.
7. `craft-engine`, `craft-casing`, `craft-payload`, `craft-resin` — assemble the finals.
8. `craft-rocket` — win.

`help` lists everything. `start` (or just begin a chat) opens the live dashboard pane and prints help. To define your own multi-step command, type `create-command`. To learn about Digital Objects themselves, ask `consult-docs` (e.g. `consult-docs what is a digital object?`).

## Where the state lives

Your inventory is local: every object is a `.dobj` file under `~/.dobj/objects/`. The driver daemon (`dobjd`) runs in the background, manages those files, generates proofs, and talks to the hosted synchronizer + relayer for chain anchoring. Nothing about your inventory leaves your machine in cleartext — only proof commitments and nullifiers go on-chain.
