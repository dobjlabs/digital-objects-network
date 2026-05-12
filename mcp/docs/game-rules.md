# bitcraft — game rules

bitcraft is a crafting game where every item you own is private. The world doesn't see your inventory; the chain only sees that *something valid* happened. You play by chopping, refining, and combining objects up a small tech tree.

## The tech tree

```
  FindLog
    │
    ▼
   Log ──► CraftWood ──► Wood ──► CraftSticks ──► Stick
                          │                         │
                          └─────────────┬───────────┘
                                        ▼
                                 CraftWoodPick
                                        │
                                        ▼
                                    WoodPick ──► MineStoneWithWoodPick ──► Stone
                                                                            │
                                                                Stick ──────┤
                                                                            ▼
                                                                     CraftStonePick
                                                                            │
                                                                            ▼
                                                                       StonePick ──► MineStoneWithStonePick ──► Stone
```

The starting move is `chop-log` — it produces a Log from nothing (you prove a small amount of computational work). Every other action consumes existing objects and produces new ones.

## Actions

| Command | Consumes | Produces |
|---|---|---|
| `chop-log` | (none — small proof-of-work) | 1 Log |
| `craft-wood` | 1 Log | 1 Wood |
| `craft-sticks` | 1 Wood | 2 Sticks |
| `craft-wood-pick` | 1 Wood + 1 Stick | 1 WoodPick |
| `mine-stone` (with WoodPick) | 1 WoodPick | 1 Stone (pick is consumed) |
| `craft-stone-pick` | 1 Stone + 1 Stick | 1 StonePick |
| `mine-stone` (with StonePick) | 1 StonePick | 1 Stone (pick is consumed) |

When you ask `mine-stone` to mine, it asks which pick you'd like to spend.

## Durability

Right now, picks are single-use: one Stone per pick. Future plugins can change this — a `.pexe` can introduce classes with durability fields that decrement on use, letting one pick mine multiple Stones. The `craft-basics` plugin keeps it simple to start.

## Why proofs

Every action produces a small zero-knowledge proof attached to the new object. The proof says "this object was reached by a valid sequence of actions" without revealing what those actions were or which objects were consumed. The chain stores the proof's commitment; observers can't tell what you have, only that your moves were legal.

A nullifier is published each time you consume an object. It prevents double-spending (no replaying the same Wood into two different picks), but doesn't reveal which object was consumed.

## How to play

Type any command name at the prompt:

- `chop-log` to start
- `craft-wood` to refine your log
- `craft-sticks` for handles
- `craft-wood-pick` to combine wood + stick
- `mine-stone` to mine
- `craft-stone-pick` for the upgrade

`help` lists everything. `start` (or just begin a chat) opens the live dashboard pane and prints help.

To define your own command (combine several steps, chain across commands), type `create-command`. To learn about Digital Objects themselves, type `digital-objects`.

## Where the state lives

Your inventory is local: every object is a `.dobj` file under `~/.dobj/objects/`. The driver daemon (`dobjd`) runs in the background, manages those files, generates proofs, and talks to the hosted synchronizer + relayer for chain anchoring. Nothing about your inventory leaves your machine in cleartext — only proof commitments and nullifiers go on-chain.
