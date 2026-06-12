# Working with Digital Objects

A Digital Objects application is any plugin loaded into the daemon. Its object
classes are the kinds of object; its actions are the commands. This file is the
generic framing -- the specific recipes come from whatever plugin is installed
(ask the daemon with `list_actions` / `list_classes`).

## The loop

1. Create: run actions that take no input to produce base objects.
2. Transform and combine: run actions that consume objects to produce new ones.
3. Repeat up the plugin's chain of actions until you reach whatever it treats as
   a finished object.

`list_objects` is your current set of objects. `check_feasibility` tells you
what you can make right now and what you are missing.

## Objects are private files

Every object you own is a `.dobj` file under `~/.dobj/objects/`, held only by
you. There is no shared server inventory. The driver daemon (`dobjd`) manages
those files, generates proofs, and talks to a synchronizer and relayer for chain
anchoring.

## Why proofs

Each action attaches a small zero-knowledge proof to the object it produces. The
proof certifies "this object was reached by a legal sequence of actions" without
revealing which actions ran or which objects were consumed. The chain stores
only the proof's commitment, so observers can tell a valid transition happened
but not what you hold.

## Nullifiers and double-spending

Consuming an object publishes a nullifier -- a one-way marker that prevents
spending the same object twice. The nullifier does not reveal which object it
came from.

## Liveness

An object is `live` once its creating action is confirmed on-chain, `pending`
while the proof is settling, and `nullified` once it has been consumed. You can
only use live objects as inputs: run an action, wait for it to confirm, and then
the outputs become live while the inputs become nullified.
