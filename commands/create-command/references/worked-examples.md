# Worked examples — translating intent into a real skill body

Two patterns, same intent rendered both ways, plus an anti-example. Use these as templates when designing a new command body in step 4 of SKILL.md.

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
