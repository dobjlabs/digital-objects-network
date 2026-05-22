---
name: bitcraft-craft-slag
description: Forge steel in the blast furnace — faster, yields slag.
---

# craft-slag

## Output rules

- Plain text only. No markdown bold, italics, bullets, code fences, or headers in user-facing output.
- No preamble. No closing summary. No suggestions. No commentary.
- Do not mention any other command, skill, or capability.

## Steps

1. Call `list_inventory`. Filter to live objects with `class_name == "Ingot"`. If fewer than 3, output exactly `no Ingot available — run craft-ingot` and stop.

2. Output candidates and prompt — exactly:

   ```
   1) <file_name of first live Ingot>
   2) <file_name of second live Ingot>
   ...
   pick 3 Ingot (comma-separated, e.g. 1,2):
   ```

   End the turn and wait for the user's reply.

3. Exit-word check (`cancel`/`quit`/`exit`/`q`/`nevermind` → `cancelled`). Otherwise parse the reply as exactly 3 comma-separated integers, all distinct, each in the valid range. Invalid → `invalid choice` and stop. Save the chosen paths in order as `<ingot_paths>`.

4. Call `run_action` with `action_id="CraftSteelBlast"` and `input_object_paths=[...<ingot_paths>]` (flatten into a single list in the order shown).

5. On success, for each entry in the tool result's `outputs` array, output one line:

   `<class_name> → <output_path>`

   The class names you should see, in order: 2× Steel, 1× Slag.

6. On tool error, output the tool's error message verbatim, on one line. Stop.
