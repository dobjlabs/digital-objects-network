# How to play a Digital Objects world

A Digital Objects "game" is any plugin loaded into the driver. Its object
classes are the items; its actions are the commands. This file is the generic
framing -- the specific recipes come from whatever plugin is installed (ask the
driver with `list_actions` / `list_classes`).

## The loop

1. Gather: run actions that take no input (mining, foraging, discovering) to
   create raw items.
2. Refine and combine: run actions that consume items to produce better ones.
3. Repeat up the plugin's tech tree until you reach whatever it treats as a
   finished good.

`list_objects` is your inventory. `check_feasibility` tells you what you can
make right now and what you are missing.

## Items are private files

Every item you own is a `.dobj` file under `~/.dobj/objects/`, held only by
you. There is no shared server inventory. The driver daemon (`dobjd`) manages
those files, generates proofs, and talks to a synchronizer and relayer for
chain anchoring.

## Why proofs

Each action attaches a small zero-knowledge proof to the item it produces. The
proof certifies "this item was reached by a legal sequence of actions" without
revealing which actions ran or which items were consumed. The chain stores only
the proof's commitment, so observers can tell a valid move happened but not what
you hold.

## Nullifiers and double-spending

Consuming an item publishes a nullifier -- a one-way marker that prevents
spending the same item twice (you cannot replay one Wood into two recipes). The
nullifier does not reveal which item it came from.

## Liveness

An item is `live` once its creating action is confirmed on-chain, `pending`
while the proof is settling, and `nullified` once it has been consumed. You can
only use live items as inputs: run an action, wait for it to confirm, and then
the outputs become live while the inputs become nullified.
