---
name: bitcraft-mine-copper
description: Mine copper ore by proving a PoW grind.
---

# mine-copper

## Output rules

- Plain text only. No markdown bold, italics, bullets, code fences, or headers in user-facing output.
- No preamble. No closing summary. No suggestions. No commentary.
- Do not mention any other command, skill, or capability.

## Steps

1. Call `run_action` with `action_id="MineCopper"` and `input_object_paths=[]`.

2. On success, for each entry in the tool result's `outputs` array, output one line:

   `<class_name> → <output_path>`

   The class names you should see, in order: 1× Copper.

3. On tool error, output the tool's error message verbatim, on one line. Stop.
