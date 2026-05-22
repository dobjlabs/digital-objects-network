---
name: bitcraft-craft-bearing
description: Mill 1 steel + 2 grease into 2 bearings.
---

# craft-bearing

## Output rules

- Plain text only. No markdown bold, italics, bullets, code fences, or headers in user-facing output.
- No preamble. No closing summary. No suggestions. No commentary.
- Do not mention any other command, skill, or capability.

## Steps

1. Call `list_inventory`. Filter to live objects with `class_name == "Steel"`. If fewer than 1, output exactly `no Steel available — run craft-steel` and stop.

2. Output candidates and prompt — exactly:

   ```
   1) <file_name of first live Steel>
   2) <file_name of second live Steel>
   ...
   pick Steel:
   ```

   End the turn and wait for the user's reply.

3. Exit-word check (`cancel`/`quit`/`exit`/`q`/`nevermind` → `cancelled`). Otherwise parse as integer. Invalid → `invalid choice` and stop. Save the chosen path as `<steel_path>`.

4. Call `list_inventory`. Filter to live objects with `class_name == "Grease"`. If fewer than 2, output exactly `no Grease available — run craft-grease` and stop.

5. Output candidates and prompt — exactly:

   ```
   1) <file_name of first live Grease>
   2) <file_name of second live Grease>
   ...
   pick 2 Grease (comma-separated, e.g. 1,2):
   ```

   End the turn and wait for the user's reply.

6. Exit-word check (`cancel`/`quit`/`exit`/`q`/`nevermind` → `cancelled`). Otherwise parse the reply as exactly 2 comma-separated integers, all distinct, each in the valid range. Invalid → `invalid choice` and stop. Save the chosen paths in order as `<grease_paths>`.

7. Call `run_action` with `action_id="CraftBearing"` and `input_object_paths=[<steel_path>, ...<grease_paths>]` (flatten into a single list in the order shown).

8. On success, for each entry in the tool result's `outputs` array, output one line:

   `<class_name> → <output_path>`

   The class names you should see, in order: 2× Bearing.

9. On tool error, output the tool's error message verbatim, on one line. Stop.
