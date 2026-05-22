---
name: bitcraft-craft-pressure-valve
description: Build a pressure valve from oil + gear.
---

# craft-pressure-valve

## Output rules

- Plain text only. No markdown bold, italics, bullets, code fences, or headers in user-facing output.
- No preamble. No closing summary. No suggestions. No commentary.
- Do not mention any other command, skill, or capability.

## Steps

1. Call `list_inventory`. Filter to live objects with `class_name == "Oil"`. If fewer than 2, output exactly `no Oil available — run mine-oil` and stop.

2. Output candidates and prompt — exactly:

   ```
   1) <file_name of first live Oil>
   2) <file_name of second live Oil>
   ...
   pick 2 Oil (comma-separated, e.g. 1,2):
   ```

   End the turn and wait for the user's reply.

3. Exit-word check (`cancel`/`quit`/`exit`/`q`/`nevermind` → `cancelled`). Otherwise parse the reply as exactly 2 comma-separated integers, all distinct, each in the valid range. Invalid → `invalid choice` and stop. Save the chosen paths in order as `<oil_paths>`.

4. Call `list_inventory`. Filter to live objects with `class_name == "Gear"`. If fewer than 1, output exactly `no Gear available — run craft-gear` and stop.

5. Output candidates and prompt — exactly:

   ```
   1) <file_name of first live Gear>
   2) <file_name of second live Gear>
   ...
   pick Gear:
   ```

   End the turn and wait for the user's reply.

6. Exit-word check (`cancel`/`quit`/`exit`/`q`/`nevermind` → `cancelled`). Otherwise parse as integer. Invalid → `invalid choice` and stop. Save the chosen path as `<gear_path>`.

7. Call `run_action` with `action_id="CraftPressureValve"` and `input_object_paths=[...<oil_paths>, <gear_path>]` (flatten into a single list in the order shown).

8. On success, for each entry in the tool result's `outputs` array, output one line:

   `<class_name> → <output_path>`

   The class names you should see, in order: 1× PressureValve.

9. On tool error, output the tool's error message verbatim, on one line. Stop.
