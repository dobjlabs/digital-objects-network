---
name: bitcraft-craft-stone-pick
description: Combine one Stone and one Stick into a StonePick.
---

# craft-stone-pick

## Output rules

- Plain text only. No markdown bold, italics, bullets, code fences, or headers in user-facing output.
- No preamble. No closing summary. No suggestions. No commentary.
- Do not mention any other command, skill, or capability.

## Steps

1. Call `list_inventory`. Build two lists: live `Stone` and live `Stick`.
2. If zero live Stone, output exactly and stop:

   `no Stone available — run mine-stone`

3. If zero live Stick, output exactly and stop:

   `no Stick available — run craft-sticks`

4. Output Stone candidates and prompt — exactly:

   ```
   1) <file_name of first live Stone>
   2) <file_name of second live Stone>
   ...
   pick Stone:
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
8. Call `run_action` with `action_id="CraftStonePick"` and `input_object_paths=[<chosen Stone path>, <chosen Stick path>]`.
9. On success, output exactly one line and stop:

   `StonePick → <output_path>`

10. On tool error, output the tool's error message verbatim, on one line. Stop.
