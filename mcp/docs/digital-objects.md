# Digital Objects — what they are

In bitcraft, everything you own is a **Digital Object**: an Iron ore, a refined Ingot, a Wire, a Circuit, a DrillBit. Each one is a small file on your machine, and it carries a cryptographic proof of where it came from.

## How they look on your machine

Each object is a JSON file at `~/.dobj/objects/<id>.dobj`. The filename includes a unique hash so two objects of the same class don't collide. The file contains:

- The object's **class** (e.g. `Iron`, `Ingot`, `Circuit`, `DrillBit`)
- Application **fields** — for a DrillBit that's `{durability: 5}`, for an Iron that's `{blueprint: "Iron", key: 0xabc…}`
- A **zero-knowledge proof** — a constant-size blob that vouches for the object's history without revealing it

`bitcraft help` lists every command. `inventory` (run from your shell as `dobj inventory`, or via the live dashboard pane) lists every object you currently hold.

## Live vs. spent

Each object is either **live** (you can use it as an input to a future action) or **spent** (you used it as input to a past action; it can't be used again).

- A new object is live.
- When you craft or mine and the recipe consumes the input, that input becomes spent. Spent objects move to `~/.dobj/objects/nullified/` and stop showing up in `inventory`.
- The chain learns a **nullifier** — a hash that uniquely identifies the spend — when an object is consumed. This is what prevents you from re-using the same Steel in two different circuits.

The nullifier doesn't tell anyone *which* object was consumed; just that *some* legal consumption happened.

## What the proof is doing

When you craft an Ingot from an Iron, the action:

1. Reads the Iron file (consuming it).
2. Builds a new Ingot object with appropriate fields.
3. Generates a small proof saying "I had a valid Iron, and CraftIngot's rules were satisfied to produce this Ingot." The proof references its inputs by their hashes, all the way back to the original mine.
4. The Ingot file is written; the Iron is moved to nullified; the nullifier is published.

The proof is **constant-size** — about 200 KB regardless of how deep your crafting history goes. A Rocket assembled from an Engine + Casing + Payload + Resin (each of which descends through five tiers of crafting) still carries one proof, not dozens.

## Why local

There is no central database of "who owns what." The chain only ever sees:
- A growing set of **transaction commitments** (one per craft action).
- A growing set of **nullifiers** (one per spent object).

No one else can read your inventory. You can move a `.dobj` file to a different machine and it's still valid — the proof speaks for itself, the chain confirms it hasn't been double-spent.

## Trading

Two players can trade by handing each other `.dobj` files (over Discord, email, USB stick — anything). The receiving party verifies the proof + checks the nullifier hasn't been published, and the trade is settled. No mediator needed.

## How this connects to commands

- `mine-iron`, `craft-ingot`, `craft-rocket`, etc. — each command produces or consumes Digital Objects.
- The preview pane (`preview`) shows your current inventory updating live as you act.
- The driver (`dobjd`) is what writes `.dobj` files, generates proofs, and manages the local state.

To learn the gameplay rules (the tech tree, what consumes what), ask `consult-docs` (e.g. `consult-docs what's the tech tree?`).
