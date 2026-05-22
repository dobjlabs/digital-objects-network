---
name: bitcraft-craft-cracking-unit
description: Build a cracking unit (requires MachineI).
---

# craft-cracking-unit

## Output rules

- Plain text only. No markdown bold, italics, bullets, code fences, or headers in user-facing output.
- No preamble. No closing summary. No suggestions. No commentary.
- Do not mention any other command, skill, or capability.

## Steps

1. Call `list_inventory`. Filter to live objects with `class_name == "Acid"`. If fewer than 4, output exactly `no Acid available — run craft-acid` and stop.

2. Output candidates and prompt — exactly:

   ```
   1) <file_name of first live Acid>
   2) <file_name of second live Acid>
   ...
   pick 4 Acid (comma-separated, e.g. 1,2):
   ```

   End the turn and wait for the user's reply.

3. Exit-word check (`cancel`/`quit`/`exit`/`q`/`nevermind` → `cancelled`). Otherwise parse the reply as exactly 4 comma-separated integers, all distinct, each in the valid range. Invalid → `invalid choice` and stop. Save the chosen paths in order as `<acid_paths>`.

4. Call `list_inventory`. Filter to live objects with `class_name == "Bearing"`. If fewer than 3, output exactly `no Bearing available — run craft-bearing` and stop.

5. Output candidates and prompt — exactly:

   ```
   1) <file_name of first live Bearing>
   2) <file_name of second live Bearing>
   ...
   pick 3 Bearing (comma-separated, e.g. 1,2):
   ```

   End the turn and wait for the user's reply.

6. Exit-word check (`cancel`/`quit`/`exit`/`q`/`nevermind` → `cancelled`). Otherwise parse the reply as exactly 3 comma-separated integers, all distinct, each in the valid range. Invalid → `invalid choice` and stop. Save the chosen paths in order as `<bearing_paths>`.

7. Call `list_inventory`. Filter to live objects with `class_name == "Grease"`. If fewer than 3, output exactly `no Grease available — run craft-grease` and stop.

8. Output candidates and prompt — exactly:

   ```
   1) <file_name of first live Grease>
   2) <file_name of second live Grease>
   ...
   pick 3 Grease (comma-separated, e.g. 1,2):
   ```

   End the turn and wait for the user's reply.

9. Exit-word check (`cancel`/`quit`/`exit`/`q`/`nevermind` → `cancelled`). Otherwise parse the reply as exactly 3 comma-separated integers, all distinct, each in the valid range. Invalid → `invalid choice` and stop. Save the chosen paths in order as `<grease_paths>`.

10. Call `run_action` with `action_id="CraftCrackingUnit"` and `input_object_paths=[...<acid_paths>, ...<bearing_paths>, ...<grease_paths>]` (flatten into a single list in the order shown).

11. On success, for each entry in the tool result's `outputs` array, output one line:

   `<class_name> → <output_path>`

   The class names you should see, in order: 1× CrackingUnit.

12. On tool error, output the tool's error message verbatim, on one line. Stop.
