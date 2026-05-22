# Worked examples — translating intent into a real skill body

Two patterns, same intent rendered both ways, plus an anti-example. Use these as templates when designing a new command body in step 3 of SKILL.md.

## Pattern A — interactive picker (no arguments)

**Intent:** "print the information contained in an object's file in a user-readable way, not including the proof"

This pattern is good when the user typically doesn't know the exact identifier up front. The command lists inventory and asks the user to pick.

```
---
name: bitcraft-inspect-object
description: Inspect a Digital Object — show its contents, omitting the ZK proof.
---

# inspect-object

## Output rules

- Plain text. One field per line as `<key>: <value>`.
- No markdown bullets, bold, or tables.
- Truncate hex values longer than 40 chars to `<first 8>…<last 6>`.
- Skip any field whose name contains "proof" or whose value is longer than 200 chars.

## Steps

1. Call `list_inventory`. If the array is empty, output exactly `no objects in inventory` and stop.

2. Print each object on its own line as `<n>) <file_name>` (n starting at 1), then `pick:` on a new line. End the turn and wait.

3. Parse the user's reply as an integer. If it doesn't match a listed n, output `invalid choice` and stop.

4. Call `inspect_object` with `file_name = <chosen object's file_name>`. The response is JSON.

5. Output these fields, each on its own line, in this order (omit any field that is missing):
   - `id: <id>`  (truncate hex)
   - `class: <className>`
   - `status: <status>`
   - `txHash: <txHash>`  (truncate hex)
   - one line per entry in `state`: `state.<key>: <value>`  (truncate hex; skip if value > 200 chars)
   - `predicate:` followed by `predicateSource` indented 2 spaces on subsequent lines

6. On tool error, output the error message verbatim, on one line. Stop.
```

## Pattern B — argument-based (no prompting)

Same intent, but invoked with the file_name as an argument: e.g. `/bitcraft-inspect-object log_0xd4…819f.dobj`. Useful when the user already knows which object they want or is scripting.

```
---
name: bitcraft-inspect-object
description: Inspect a Digital Object — show its contents, omitting the ZK proof.
argument-hint: [file_name]
arguments: file_name
---

# inspect-object

## Output rules

- Plain text. One field per line as `<key>: <value>`.
- No markdown bullets, bold, or tables.
- Truncate hex values longer than 40 chars to `<first 8>…<last 6>`.
- Skip any field whose name contains "proof" or whose value is longer than 200 chars.

## Steps

1. If `$file_name` is empty, output `usage: inspect-object [file_name]` and stop.

2. Call `inspect_object` with `file_name = $file_name`. The response is JSON. On tool error (file not found, etc.), output the error message verbatim and stop.

3. Output these fields, each on its own line, in this order (omit any field that is missing):
   - `id: <id>`  (truncate hex)
   - `class: <className>`
   - `status: <status>`
   - `txHash: <txHash>`  (truncate hex)
   - one line per entry in `state`: `state.<key>: <value>`  (truncate hex; skip if value > 200 chars)
   - `predicate:` followed by `predicateSource` indented 2 spaces on subsequent lines
```

## Pattern C — multi-step planner that walks the recipe tree backwards

**Intent:** "make a Steel — figure out what's needed, reuse anything already in inventory, mine and craft the rest. Use base recipes only, no specialization variants."

This pattern fits commands whose job is to *reach a target class*: the body walks the recipe tree backwards from the target, checks inventory at each level, mines / crafts only what's missing, and finally produces the target. No interactive picker — the agent consumes the oldest matching objects automatically.

"Base recipe only" means: when a class has multiple producing actions, pick the one with the simplest input list — no station gates (`-blast`, `-fabbed`, `-cracked`, `-cast`), no tool durability (`-drilled`, `-soldered`, `-pressurized`), no recipe shifts (`-flash`, `-crude`, `-flux`, `-lye`), no chamber stabilization (`-stable`, `-tuned`). For example: `CraftSteel` not `CraftSteelBlast`; `CraftIngot` not `CraftIngotFlux` or `CraftIngotDrilled`; `CraftAcid` not `CraftAcidFlash` or `CraftAcidCrude`.

```
---
name: bitcraft-make-steel
description: Make a Steel — reuse inventory, mine and craft missing inputs end-to-end. Base recipes only.
---

# make-steel

## Output rules

- Plain text. One plan line per class. One execution line per action call.
- Plan lines look like `<Class> have:<N> need:<M>`.
- Execution lines look like `<action_id> → <output_path>` (one line per output).
- No markdown bullets, bold, or tables. No commentary outside these lines.

## Recipe chain (base only — DO NOT substitute specialization variants)

| Target | action_id    | Inputs   | Outputs  |
|--------|--------------|----------|----------|
| Iron   | `MineIron`   | (none)   | 1 Iron   |
| Ingot  | `CraftIngot` | 1 Iron   | 1 Ingot  |
| Steel  | `CraftSteel` | 3 Ingot  | 2 Steel  |

## Steps

1. Call `list_inventory`. From the response, build:
   - `iron_paths`  — list of `file_path` for every LIVE Iron, in inventory order
   - `ingot_paths` — list of `file_path` for every LIVE Ingot, in inventory order

2. Compute the plan (one Steel = 3 Ingot = 3 Iron):
   - `ingot_need = max(0, 3 - len(ingot_paths))`           — extra Ingot to craft
   - `iron_need  = max(0, ingot_need - len(iron_paths))`   — extra Iron to mine beyond what's already on hand

3. Print the plan, three lines, in this exact form:

   ```
   Iron  have:<len(iron_paths)>  need:<iron_need>
   Ingot have:<len(ingot_paths)> need:<ingot_need>
   Steel have:0                  need:1
   ```

4. Mining loop. Repeat `iron_need` times:
   - Call `run_action` with `action_id="MineIron"` and `input_object_paths=[]`.
   - Append the first entry of the result's `outputs` to `iron_paths`.
   - Output one line: `MineIron → <output_path>`.
   - On tool error, output the error message verbatim and stop the entire flow.

5. Crafting Ingot loop. Repeat `ingot_need` times, consuming one Iron each time:
   - Pop the first element of `iron_paths` as `iron_path`.
   - Call `run_action` with `action_id="CraftIngot"` and `input_object_paths=[iron_path]`.
   - Append the first entry of the result's `outputs` to `ingot_paths`.
   - Output one line: `CraftIngot → <output_path>`.
   - On tool error, output the error message verbatim and stop.

6. Final step. Pop the first 3 entries from `ingot_paths` as `[a, b, c]`. Call `run_action` with `action_id="CraftSteel"` and `input_object_paths=[a, b, c]`.

7. On success, the result's `outputs` array has 2 entries (CraftSteel produces 2 Steel). Output one line per entry:

   ```
   CraftSteel → <output_path>
   ```

8. On any tool error during steps 6–7, output the error message verbatim and stop.
```

The body never re-fetches `list_inventory` after step 1 — it tracks freshly produced object paths from each `run_action`'s `outputs` field, which is faster and avoids races. If a later command depends on more than one tier (e.g. craft Engine: needs Pistons + Gear + Circuit + Canvas, each with its own subtree), extend the recipe-chain table downward and add an inventory check + execution loop per intermediate class, in topological order.

## Anti-example — don't do this

```
---
name: bitcraft-inspect-object
description: prints the information contained in an object's file in a user-readable way, besides the proof
---

# inspect-object

print the information contained in an object's file in a user-readable way, not including the proof
```

Failures:
- The body has no concrete steps the future agent can execute.
- No MCP tool is named, so the agent will guess (and may pick the wrong one).
- No output format, so the result varies per invocation.
- No error handling, no input-picking logic.
- Description duplicates the body verbatim.
- Description starts with "prints" not "Inspect" — verb mismatch with the command name reduces matching reliability.

Always produce a draft with named tools, specified output, and explicit error cases — pick the picker pattern OR the argument pattern, but never echo the user's prose into the body.
