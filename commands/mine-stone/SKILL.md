---
name: bitcraft-mine-stone
description: Mine a Stone using a WoodPick or StonePick.
---

# mine-stone

## Output rules

- Plain text only. No markdown bold, italics, bullets, code fences, or headers in user-facing output.
- No preamble. No closing summary. No suggestions. No commentary.
- Do not mention any other command, skill, or capability.

## Steps

1. Call `list_inventory`. Build a single list of live objects whose `class_name` is `WoodPick` or `StonePick`, preserving inventory order.
2. If the list is empty, output exactly and stop:

   `no pick available — run craft-wood-pick`

3. Output candidates and prompt — exactly this format, `n` starting at 1, one line per pick:

   ```
   1) <class_name> <file_name>
   2) <class_name> <file_name>
   ...
   pick:
   ```

   End the turn. Wait for the user's reply.

4. Parse the user's reply as an integer. If invalid, output `invalid choice` and stop.
5. Determine `action_id`:
   - If the chosen pick's `class_name == "WoodPick"`: `action_id="MineStoneWithWoodPick"`
   - If `class_name == "StonePick"`: `action_id="MineStoneWithStonePick"`
6. Call `run_action` with that `action_id` and `input_object_paths=[<chosen pick path>]`.
7. On success, output exactly one line and stop:

   `Stone → <output_path>`

8. On tool error, output the tool's error message verbatim, on one line. Stop.
