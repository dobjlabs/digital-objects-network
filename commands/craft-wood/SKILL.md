---
name: bitcraft-craft-wood
description: Bitcraft command — refine one Log into a Wood object via CraftWood. Use when the user types "craft-wood", "craft wood", "make wood", or asks bitcraft to refine a Log.
---

# craft-wood

## Output rules

- Plain text only. No markdown bold, italics, bullets, code fences, or headers in user-facing output.
- No preamble. No closing summary. No suggestions. No commentary.
- Do not mention any other command, skill, or capability.

## Steps

1. Call `list_inventory`. Build the list of live objects with `class_name == "Log"`.
2. If zero live Logs, output exactly and stop:

   `no Log available — run obtain-log`

3. Otherwise, output the candidates and the prompt — exactly this format, one line per candidate, `n` starting at 1:

   ```
   1) <file_name of first live Log>
   2) <file_name of second live Log>
   ...
   pick Log:
   ```

   Then end the turn and wait for the user's reply.

4. When the user replies, parse a single integer. If it does not match a listed index, output exactly and stop:

   `invalid choice`

5. Call `run_action` with `action_id="CraftWood"` and `input_object_paths=[<file_path of the chosen Log>]`.
6. On success, output exactly one line and stop:

   `Wood → <output_path>`

7. On tool error, output the tool's error message verbatim, on one line. Stop.
