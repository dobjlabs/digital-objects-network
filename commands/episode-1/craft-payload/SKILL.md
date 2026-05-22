---
name: bitcraft-craft-payload
description: Build a payload from panels, circuit, canvas, wire, grease.
---

# craft-payload

## Output rules

- Plain text only. No markdown bold, italics, bullets, code fences, or headers in user-facing output.
- No preamble. No closing summary. No suggestions. No commentary.
- Do not mention any other command, skill, or capability.

## Steps

1. Call `list_inventory`. Filter to live objects with `class_name == "Panel"`. If fewer than 3, output exactly `no Panel available — run craft-panel` and stop.

2. Output candidates and prompt — exactly:

   ```
   1) <file_name of first live Panel>
   2) <file_name of second live Panel>
   ...
   pick 3 Panel (comma-separated, e.g. 1,2):
   ```

   End the turn and wait for the user's reply.

3. Exit-word check (`cancel`/`quit`/`exit`/`q`/`nevermind` → `cancelled`). Otherwise parse the reply as exactly 3 comma-separated integers, all distinct, each in the valid range. Invalid → `invalid choice` and stop. Save the chosen paths in order as `<panel_paths>`.

4. Call `list_inventory`. Filter to live objects with `class_name == "Circuit"`. If fewer than 1, output exactly `no Circuit available — run craft-circuit` and stop.

5. Output candidates and prompt — exactly:

   ```
   1) <file_name of first live Circuit>
   2) <file_name of second live Circuit>
   ...
   pick Circuit:
   ```

   End the turn and wait for the user's reply.

6. Exit-word check (`cancel`/`quit`/`exit`/`q`/`nevermind` → `cancelled`). Otherwise parse as integer. Invalid → `invalid choice` and stop. Save the chosen path as `<circuit_path>`.

7. Call `list_inventory`. Filter to live objects with `class_name == "Canvas"`. If fewer than 1, output exactly `no Canvas available — run craft-canvas` and stop.

8. Output candidates and prompt — exactly:

   ```
   1) <file_name of first live Canvas>
   2) <file_name of second live Canvas>
   ...
   pick Canvas:
   ```

   End the turn and wait for the user's reply.

9. Exit-word check (`cancel`/`quit`/`exit`/`q`/`nevermind` → `cancelled`). Otherwise parse as integer. Invalid → `invalid choice` and stop. Save the chosen path as `<canvas_path>`.

10. Call `list_inventory`. Filter to live objects with `class_name == "Wire"`. If fewer than 1, output exactly `no Wire available — run craft-wire` and stop.

11. Output candidates and prompt — exactly:

   ```
   1) <file_name of first live Wire>
   2) <file_name of second live Wire>
   ...
   pick Wire:
   ```

   End the turn and wait for the user's reply.

12. Exit-word check (`cancel`/`quit`/`exit`/`q`/`nevermind` → `cancelled`). Otherwise parse as integer. Invalid → `invalid choice` and stop. Save the chosen path as `<wire_path>`.

13. Call `list_inventory`. Filter to live objects with `class_name == "Grease"`. If fewer than 1, output exactly `no Grease available — run craft-grease` and stop.

14. Output candidates and prompt — exactly:

   ```
   1) <file_name of first live Grease>
   2) <file_name of second live Grease>
   ...
   pick Grease:
   ```

   End the turn and wait for the user's reply.

15. Exit-word check (`cancel`/`quit`/`exit`/`q`/`nevermind` → `cancelled`). Otherwise parse as integer. Invalid → `invalid choice` and stop. Save the chosen path as `<grease_path>`.

16. Call `run_action` with `action_id="CraftPayload"` and `input_object_paths=[...<panel_paths>, <circuit_path>, <canvas_path>, <wire_path>, <grease_path>]` (flatten into a single list in the order shown).

17. On success, for each entry in the tool result's `outputs` array, output one line:

   `<class_name> → <output_path>`

   The class names you should see, in order: 1× Payload.

18. On tool error, output the tool's error message verbatim, on one line. Stop.
