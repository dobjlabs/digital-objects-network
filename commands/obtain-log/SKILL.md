---
name: bitcraft-obtain-log
description: Obtain a new Log object.
---

# obtain-log

## Output rules

- Plain text only. No markdown bold, italics, bullets, code fences, or headers in user-facing output.
- No preamble. No closing summary. No suggestions. No commentary.
- Do not mention any other command, skill, or capability.

## Steps

1. Call `run_action` with `action_id="FindLog"` and `input_object_paths=[]`.
2. On success, output exactly one line and stop:

   `Log → <output_path>`

   Replace `<output_path>` with the path field from the first entry of the tool result's `outputs` array.

3. On tool error, output the tool's error message verbatim, on one line. Stop.
