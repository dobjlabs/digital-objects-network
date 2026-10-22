---
name: bitcraft-play
description: Play the installed Bitcraft crafting game through its bundled CLI. Use for inspecting inventory, choosing recipes, and executing or following game actions.
---

Use the CLI executable and game home supplied in the player's onboarding
prompt. Pass `--home PATH` before the subcommand on every invocation. The
Bitcraft desktop app must be running. Start with `--help`; the CLI connects
to the app's own daemon, without a repository checkout or a separate server.

## Discover and plan

- Read `objects --json` and `actions --json` to discover current inventory
  and available recipes. Use `inspect-action PLUGIN::ACTION --json` for
  detailed requirements and `inspect-object FILE --json` for object fields.
- Prefer objects whose status is `live` and whose `heldByRunId` is empty.
  Pending and unknown outputs are not yet usable; nullified inputs are spent.
- `feasibility PLUGIN::ACTION --json` suggests input filenames by class.
  Also check field constraints in the action's predicate, such as durability,
  level, or bucket contents. Class matching alone does not prove feasibility.
- Follow the player's goal. Discover recipes from the installed catalog;
  do not assume a fixed recipe chain or a previously held inventory.

## Execute and recover

- Run `run PLUGIN::ACTION INPUT_FILE... --json`, with input filenames in the
  order required by the action. It prints the run ID and progress to stderr,
  follows the action through confirmation, and prints its final state to
  stdout. A failed action exits nonzero and includes its error in that state.
- Keep the shell process running until completion. If the agent host supports
  background processes, do independent work while it runs, then collect its
  output and exit status. Avoid repeated model-driven status polling.
- Runs can execute concurrently when they use distinct inputs. Limit active
  work to the machine's available compute and memory. A run is complete only
  when its status is `succeeded` or `failed`.
- If following a run is interrupted, inspect `runs list --active --json`
  or `runs show RUN_ID --json`, and reattach with `runs wait RUN_ID --json`.
  Reattaching does not submit another action. Runs are kept in memory for
  about ten minutes after completion and are lost on daemon restart.
- Losing the CLI connection does not cancel the action. Inspect existing
  runs and inventory before deciding whether another submission is needed.
- Omit `grind_key` for ordinary gathering: the daemon searches for a valid
  proof-of-work key. A supplied key must satisfy the same constraints and
  does not remove the need for ZK proving and network confirmation.

Refresh inventory after completion and use the new filenames for subsequent
actions, including updated versions of tools. Report the result and any
unresolved connection or game-state uncertainty to the player.
