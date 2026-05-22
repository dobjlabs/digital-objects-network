---
name: bitcraft-craft-machine-ii
description: Build the tier-2 assembler (requires MachineI). Unlocks t4 & t5.
---

# craft-machine-ii

## Output rules

- Plain text only. No markdown bold, italics, bullets, code fences, or headers in user-facing output.
- No preamble. No closing summary. No suggestions. No commentary.
- Do not mention any other command, skill, or capability.

## Steps

1. Call `list_inventory`. Filter to live objects with `class_name == "Circuit"`. If fewer than 2, output exactly `no Circuit available — run craft-circuit` and stop.

2. Output candidates and prompt — exactly:

   ```
   1) <file_name of first live Circuit>
   2) <file_name of second live Circuit>
   ...
   pick 2 Circuit (comma-separated, e.g. 1,2):
   ```

   End the turn and wait for the user's reply.

3. Exit-word check (`cancel`/`quit`/`exit`/`q`/`nevermind` → `cancelled`). Otherwise parse the reply as exactly 2 comma-separated integers, all distinct, each in the valid range. Invalid → `invalid choice` and stop. Save the chosen paths in order as `<circuit_paths>`.

4. Call `list_inventory`. Filter to live objects with `class_name == "Bearing"`. If fewer than 2, output exactly `no Bearing available — run craft-bearing` and stop.

5. Output candidates and prompt — exactly:

   ```
   1) <file_name of first live Bearing>
   2) <file_name of second live Bearing>
   ...
   pick 2 Bearing (comma-separated, e.g. 1,2):
   ```

   End the turn and wait for the user's reply.

6. Exit-word check (`cancel`/`quit`/`exit`/`q`/`nevermind` → `cancelled`). Otherwise parse the reply as exactly 2 comma-separated integers, all distinct, each in the valid range. Invalid → `invalid choice` and stop. Save the chosen paths in order as `<bearing_paths>`.

7. Call `run_action` with `action_id="CraftMachineII"` and `input_object_paths=[...<circuit_paths>, ...<bearing_paths>]` (flatten into a single list in the order shown).

8. On success, for each entry in the tool result's `outputs` array, output one line:

   `<class_name> → <output_path>`

   The class names you should see, in order: 1× MachineII.

9. On tool error, output the tool's error message verbatim, on one line. Stop.
