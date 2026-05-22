---
name: bitcraft-craft-machine-i
description: Build the tier-1 assembler. Unlocks t3 recipes.
---

# craft-machine-i

## Output rules

- Plain text only. No markdown bold, italics, bullets, code fences, or headers in user-facing output.
- No preamble. No closing summary. No suggestions. No commentary.
- Do not mention any other command, skill, or capability.

## Steps

1. Call `list_inventory`. Filter to live objects with `class_name == "Steel"`. If fewer than 4, output exactly `no Steel available — run craft-steel` and stop.

2. Output candidates and prompt — exactly:

   ```
   1) <file_name of first live Steel>
   2) <file_name of second live Steel>
   ...
   pick 4 Steel (comma-separated, e.g. 1,2):
   ```

   End the turn and wait for the user's reply.

3. Exit-word check (`cancel`/`quit`/`exit`/`q`/`nevermind` → `cancelled`). Otherwise parse the reply as exactly 4 comma-separated integers, all distinct, each in the valid range. Invalid → `invalid choice` and stop. Save the chosen paths in order as `<steel_paths>`.

4. Call `list_inventory`. Filter to live objects with `class_name == "Gear"`. If fewer than 3, output exactly `no Gear available — run craft-gear` and stop.

5. Output candidates and prompt — exactly:

   ```
   1) <file_name of first live Gear>
   2) <file_name of second live Gear>
   ...
   pick 3 Gear (comma-separated, e.g. 1,2):
   ```

   End the turn and wait for the user's reply.

6. Exit-word check (`cancel`/`quit`/`exit`/`q`/`nevermind` → `cancelled`). Otherwise parse the reply as exactly 3 comma-separated integers, all distinct, each in the valid range. Invalid → `invalid choice` and stop. Save the chosen paths in order as `<gear_paths>`.

7. Call `list_inventory`. Filter to live objects with `class_name == "Coil"`. If fewer than 2, output exactly `no Coil available — run craft-coil` and stop.

8. Output candidates and prompt — exactly:

   ```
   1) <file_name of first live Coil>
   2) <file_name of second live Coil>
   ...
   pick 2 Coil (comma-separated, e.g. 1,2):
   ```

   End the turn and wait for the user's reply.

9. Exit-word check (`cancel`/`quit`/`exit`/`q`/`nevermind` → `cancelled`). Otherwise parse the reply as exactly 2 comma-separated integers, all distinct, each in the valid range. Invalid → `invalid choice` and stop. Save the chosen paths in order as `<coil_paths>`.

10. Call `run_action` with `action_id="CraftMachineI"` and `input_object_paths=[...<steel_paths>, ...<gear_paths>, ...<coil_paths>]` (flatten into a single list in the order shown).

11. On success, for each entry in the tool result's `outputs` array, output one line:

   `<class_name> → <output_path>`

   The class names you should see, in order: 1× MachineI.

12. On tool error, output the tool's error message verbatim, on one line. Stop.
