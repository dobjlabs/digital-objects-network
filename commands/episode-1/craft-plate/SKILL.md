---
name: bitcraft-craft-plate
description: Roll copper into a plate.
---

# craft-plate

## Output rules

- Plain text only. No markdown bold, italics, bullets, code fences, or headers in user-facing output.
- No preamble. No closing summary. No suggestions. No commentary.
- Do not mention any other command, skill, or capability.

## Steps

1. Call `list_inventory`. Filter to live objects with `class_name == "Copper"`. If fewer than 1, output exactly `no Copper available — run mine-copper` and stop.

2. Output candidates and prompt — exactly:

   ```
   1) <file_name of first live Copper>
   2) <file_name of second live Copper>
   ...
   pick Copper:
   ```

   End the turn and wait for the user's reply.

3. Exit-word check (`cancel`/`quit`/`exit`/`q`/`nevermind` → `cancelled`). Otherwise parse as integer. Invalid → `invalid choice` and stop. Save the chosen path as `<copper_path>`.

4. Call `run_action` with `action_id="CraftPlate"` and `input_object_paths=[<copper_path>]` (flatten into a single list in the order shown).

5. On success, for each entry in the tool result's `outputs` array, output one line:

   `<class_name> → <output_path>`

   The class names you should see, in order: 1× Plate.

6. On tool error, output the tool's error message verbatim, on one line. Stop.
