---
name: bitcraft-craft-blast-furnace
description: Build a blast furnace (requires MachineI).
---

# craft-blast-furnace

## Output rules

- Plain text only. No markdown bold, italics, bullets, code fences, or headers in user-facing output.
- No preamble. No closing summary. No suggestions. No commentary.
- Do not mention any other command, skill, or capability.

## Steps

1. Call `list_inventory`. Filter to live objects with `class_name == "Steel"`. If fewer than 3, output exactly `no Steel available — run craft-steel` and stop.

2. Output candidates and prompt — exactly:

   ```
   1) <file_name of first live Steel>
   2) <file_name of second live Steel>
   ...
   pick 3 Steel (comma-separated, e.g. 1,2):
   ```

   End the turn and wait for the user's reply.

3. Exit-word check (`cancel`/`quit`/`exit`/`q`/`nevermind` → `cancelled`). Otherwise parse the reply as exactly 3 comma-separated integers, all distinct, each in the valid range. Invalid → `invalid choice` and stop. Save the chosen paths in order as `<steel_paths>`.

4. Call `list_inventory`. Filter to live objects with `class_name == "Gear"`. If fewer than 2, output exactly `no Gear available — run craft-gear` and stop.

5. Output candidates and prompt — exactly:

   ```
   1) <file_name of first live Gear>
   2) <file_name of second live Gear>
   ...
   pick 2 Gear (comma-separated, e.g. 1,2):
   ```

   End the turn and wait for the user's reply.

6. Exit-word check (`cancel`/`quit`/`exit`/`q`/`nevermind` → `cancelled`). Otherwise parse the reply as exactly 2 comma-separated integers, all distinct, each in the valid range. Invalid → `invalid choice` and stop. Save the chosen paths in order as `<gear_paths>`.

7. Call `list_inventory`. Filter to live objects with `class_name == "Coil"`. If fewer than 1, output exactly `no Coil available — run craft-coil` and stop.

8. Output candidates and prompt — exactly:

   ```
   1) <file_name of first live Coil>
   2) <file_name of second live Coil>
   ...
   pick Coil:
   ```

   End the turn and wait for the user's reply.

9. Exit-word check (`cancel`/`quit`/`exit`/`q`/`nevermind` → `cancelled`). Otherwise parse as integer. Invalid → `invalid choice` and stop. Save the chosen path as `<coil_path>`.

10. Call `list_inventory`. Filter to live objects with `class_name == "Acid"`. If fewer than 2, output exactly `no Acid available — run craft-acid` and stop.

11. Output candidates and prompt — exactly:

   ```
   1) <file_name of first live Acid>
   2) <file_name of second live Acid>
   ...
   pick 2 Acid (comma-separated, e.g. 1,2):
   ```

   End the turn and wait for the user's reply.

12. Exit-word check (`cancel`/`quit`/`exit`/`q`/`nevermind` → `cancelled`). Otherwise parse the reply as exactly 2 comma-separated integers, all distinct, each in the valid range. Invalid → `invalid choice` and stop. Save the chosen paths in order as `<acid_paths>`.

13. Call `run_action` with `action_id="CraftBlastFurnace"` and `input_object_paths=[...<steel_paths>, ...<gear_paths>, <coil_path>, ...<acid_paths>]` (flatten into a single list in the order shown).

14. On success, for each entry in the tool result's `outputs` array, output one line:

   `<class_name> → <output_path>`

   The class names you should see, in order: 1× BlastFurnace.

15. On tool error, output the tool's error message verbatim, on one line. Stop.
