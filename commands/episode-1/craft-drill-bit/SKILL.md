---
name: bitcraft-craft-drill-bit
description: Forge a drill bit from iron + gear.
---

# craft-drill-bit

## Output rules

- Plain text only. No markdown bold, italics, bullets, code fences, or headers in user-facing output.
- No preamble. No closing summary. No suggestions. No commentary.
- Do not mention any other command, skill, or capability.

## Steps

1. Call `list_inventory`. Filter to live objects with `class_name == "Iron"`. If fewer than 1, output exactly `no Iron available — run mine-iron` and stop.

2. Output candidates and prompt — exactly:

   ```
   1) <file_name of first live Iron>
   2) <file_name of second live Iron>
   ...
   pick Iron:
   ```

   End the turn and wait for the user's reply.

3. Exit-word check (`cancel`/`quit`/`exit`/`q`/`nevermind` → `cancelled`). Otherwise parse as integer. Invalid → `invalid choice` and stop. Save the chosen path as `<iron_path>`.

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

7. Call `run_action` with `action_id="CraftDrillBit"` and `input_object_paths=[<iron_path>, <gear_path>]` (flatten into a single list in the order shown).

8. On success, for each entry in the tool result's `outputs` array, output one line:

   `<class_name> → <output_path>`

   The class names you should see, in order: 1× DrillBit.

9. On tool error, output the tool's error message verbatim, on one line. Stop.
