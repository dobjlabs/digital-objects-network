---
name: bitcraft-craft-panel
description: Bond 1 board + 1 extract into 2 panels (no proof).
---

# craft-panel

## Output rules

- Plain text only. No markdown bold, italics, bullets, code fences, or headers in user-facing output.
- No preamble. No closing summary. No suggestions. No commentary.
- Do not mention any other command, skill, or capability.

## Steps

1. Call `list_inventory`. Filter to live objects with `class_name == "Board"`. If fewer than 1, output exactly `no Board available — run craft-board` and stop.

2. Output candidates and prompt — exactly:

   ```
   1) <file_name of first live Board>
   2) <file_name of second live Board>
   ...
   pick Board:
   ```

   End the turn and wait for the user's reply.

3. Exit-word check (`cancel`/`quit`/`exit`/`q`/`nevermind` → `cancelled`). Otherwise parse as integer. Invalid → `invalid choice` and stop. Save the chosen path as `<board_path>`.

4. Call `list_inventory`. Filter to live objects with `class_name == "Extract"`. If fewer than 1, output exactly `no Extract available — run craft-extract` and stop.

5. Output candidates and prompt — exactly:

   ```
   1) <file_name of first live Extract>
   2) <file_name of second live Extract>
   ...
   pick Extract:
   ```

   End the turn and wait for the user's reply.

6. Exit-word check (`cancel`/`quit`/`exit`/`q`/`nevermind` → `cancelled`). Otherwise parse as integer. Invalid → `invalid choice` and stop. Save the chosen path as `<extract_path>`.

7. Call `run_action` with `action_id="CraftPanel"` and `input_object_paths=[<board_path>, <extract_path>]` (flatten into a single list in the order shown).

8. On success, for each entry in the tool result's `outputs` array, output one line:

   `<class_name> → <output_path>`

   The class names you should see, in order: 2× Panel.

9. On tool error, output the tool's error message verbatim, on one line. Stop.
