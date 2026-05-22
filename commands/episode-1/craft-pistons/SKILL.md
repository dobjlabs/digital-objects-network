---
name: bitcraft-craft-pistons
description: Assemble pistons from bearings, coil, and grease.
---

# craft-pistons

## Output rules

- Plain text only. No markdown bold, italics, bullets, code fences, or headers in user-facing output.
- No preamble. No closing summary. No suggestions. No commentary.
- Do not mention any other command, skill, or capability.

## Steps

1. Call `list_inventory`. Filter to live objects with `class_name == "Bearing"`. If fewer than 2, output exactly `no Bearing available — run craft-bearing` and stop.

2. Output candidates and prompt — exactly:

   ```
   1) <file_name of first live Bearing>
   2) <file_name of second live Bearing>
   ...
   pick 2 Bearing (comma-separated, e.g. 1,2):
   ```

   End the turn and wait for the user's reply.

3. Exit-word check (`cancel`/`quit`/`exit`/`q`/`nevermind` → `cancelled`). Otherwise parse the reply as exactly 2 comma-separated integers, all distinct, each in the valid range. Invalid → `invalid choice` and stop. Save the chosen paths in order as `<bearing_paths>`.

4. Call `list_inventory`. Filter to live objects with `class_name == "Coil"`. If fewer than 2, output exactly `no Coil available — run craft-coil` and stop.

5. Output candidates and prompt — exactly:

   ```
   1) <file_name of first live Coil>
   2) <file_name of second live Coil>
   ...
   pick 2 Coil (comma-separated, e.g. 1,2):
   ```

   End the turn and wait for the user's reply.

6. Exit-word check (`cancel`/`quit`/`exit`/`q`/`nevermind` → `cancelled`). Otherwise parse the reply as exactly 2 comma-separated integers, all distinct, each in the valid range. Invalid → `invalid choice` and stop. Save the chosen paths in order as `<coil_paths>`.

7. Call `list_inventory`. Filter to live objects with `class_name == "Grease"`. If fewer than 1, output exactly `no Grease available — run craft-grease` and stop.

8. Output candidates and prompt — exactly:

   ```
   1) <file_name of first live Grease>
   2) <file_name of second live Grease>
   ...
   pick Grease:
   ```

   End the turn and wait for the user's reply.

9. Exit-word check (`cancel`/`quit`/`exit`/`q`/`nevermind` → `cancelled`). Otherwise parse as integer. Invalid → `invalid choice` and stop. Save the chosen path as `<grease_path>`.

10. Call `run_action` with `action_id="CraftPistons"` and `input_object_paths=[...<bearing_paths>, ...<coil_paths>, <grease_path>]` (flatten into a single list in the order shown).

11. On success, for each entry in the tool result's `outputs` array, output one line:

   `<class_name> → <output_path>`

   The class names you should see, in order: 1× Pistons.

12. On tool error, output the tool's error message verbatim, on one line. Stop.
