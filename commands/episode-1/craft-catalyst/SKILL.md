---
name: bitcraft-craft-catalyst
description: Combine 3 sludge + 1 wire into a catalyst.
---

# craft-catalyst

## Output rules

- Plain text only. No markdown bold, italics, bullets, code fences, or headers in user-facing output.
- No preamble. No closing summary. No suggestions. No commentary.
- Do not mention any other command, skill, or capability.

## Steps

1. Call `list_inventory`. Filter to live objects with `class_name == "Sludge"`. If fewer than 3, output exactly `no Sludge available — run craft-sludge` and stop.

2. Output candidates and prompt — exactly:

   ```
   1) <file_name of first live Sludge>
   2) <file_name of second live Sludge>
   ...
   pick 3 Sludge (comma-separated, e.g. 1,2):
   ```

   End the turn and wait for the user's reply.

3. Exit-word check (`cancel`/`quit`/`exit`/`q`/`nevermind` → `cancelled`). Otherwise parse the reply as exactly 3 comma-separated integers, all distinct, each in the valid range. Invalid → `invalid choice` and stop. Save the chosen paths in order as `<sludge_paths>`.

4. Call `list_inventory`. Filter to live objects with `class_name == "Wire"`. If fewer than 1, output exactly `no Wire available — run craft-wire` and stop.

5. Output candidates and prompt — exactly:

   ```
   1) <file_name of first live Wire>
   2) <file_name of second live Wire>
   ...
   pick Wire:
   ```

   End the turn and wait for the user's reply.

6. Exit-word check (`cancel`/`quit`/`exit`/`q`/`nevermind` → `cancelled`). Otherwise parse as integer. Invalid → `invalid choice` and stop. Save the chosen path as `<wire_path>`.

7. Call `run_action` with `action_id="CraftCatalyst"` and `input_object_paths=[...<sludge_paths>, <wire_path>]` (flatten into a single list in the order shown).

8. On success, for each entry in the tool result's `outputs` array, output one line:

   `<class_name> → <output_path>`

   The class names you should see, in order: 1× Catalyst.

9. On tool error, output the tool's error message verbatim, on one line. Stop.
