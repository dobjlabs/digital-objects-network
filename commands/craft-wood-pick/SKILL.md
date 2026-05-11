---
name: bitcraft-craft-wood-pick
description: Bitcraft command — combine one Wood and one Stick into a WoodPick via CraftWoodPick. Use when the user types "craft-wood-pick", "craft wood pick", "make a wood pick", or asks bitcraft to craft a WoodPick.
---

# craft-wood-pick

## Output rules

- Plain text only. No markdown bold, italics, bullets, code fences, or headers in user-facing output.
- No preamble. No closing summary. No suggestions. No commentary.
- Do not mention any other command, skill, or capability.

## Steps

1. Call `list_inventory`. Build two lists: live `Wood` and live `Stick`.
2. If zero live Wood, output exactly and stop:

   `no Wood available — run craft-wood`

3. If zero live Stick, output exactly and stop:

   `no Stick available — run craft-sticks`

4. Output Wood candidates and prompt — exactly:

   ```
   1) <file_name of first live Wood>
   2) <file_name of second live Wood>
   ...
   pick Wood:
   ```

   End the turn. Wait for reply.

5. Parse the user's reply as an integer. If invalid, output `invalid choice` and stop.
6. Output Stick candidates and prompt — exactly:

   ```
   1) <file_name of first live Stick>
   2) <file_name of second live Stick>
   ...
   pick Stick:
   ```

   End the turn. Wait for reply.

7. Parse the user's reply as an integer. If invalid, output `invalid choice` and stop.
8. Call `run_action` with `action_id="CraftWoodPick"` and `input_object_paths=[<chosen Wood path>, <chosen Stick path>]`.
9. On success, output exactly one line and stop:

   `WoodPick → <output_path>`

10. On tool error, output the tool's error message verbatim, on one line. Stop.
