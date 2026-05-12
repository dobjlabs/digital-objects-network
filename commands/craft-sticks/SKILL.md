---
name: bitcraft-craft-sticks
description: Split one Wood into two Stick objects.
---

# craft-sticks

## Output rules

- Plain text only. No markdown bold, italics, bullets, code fences, or headers in user-facing output.
- No preamble. No closing summary. No suggestions. No commentary.
- Do not mention any other command, skill, or capability.

## Steps

1. Call `list_inventory`. Build the list of live objects with `class_name == "Wood"`.
2. If zero live Wood, output exactly and stop:

   `no Wood available — run craft-wood`

3. Otherwise, output candidates and prompt — exactly this format, `n` starting at 1:

   ```
   1) <file_name of first live Wood>
   2) <file_name of second live Wood>
   ...
   pick Wood:
   ```

   End the turn and wait for the user's reply.

4. When the user replies, first check for exit words. If the reply (case-insensitive, trimmed) is `cancel`, `quit`, `exit`, `q`, or `nevermind`, output exactly `cancelled` and stop. Otherwise parse the reply as a single integer. If invalid, output exactly and stop:

   `invalid choice`

5. Call `run_action` with `action_id="CraftSticks"` and `input_object_paths=[<file_path of the chosen Wood>]`.
6. On success, output exactly two lines (one per output entry) and stop:

   ```
   Stick → <output_path_1>
   Stick → <output_path_2>
   ```

7. On tool error, output the tool's error message verbatim, on one line. Stop.
