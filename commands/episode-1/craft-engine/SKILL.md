---
name: bitcraft-craft-engine
description: Craft Engine (2 recipe variants).
---

# craft-engine

## Output rules

- Plain text only. No markdown bold, italics, bullets, code fences, or headers in user-facing output.
- No preamble. No closing summary. No suggestions. No commentary.
- Do not mention any other command, skill, or capability.

## Steps

1. Output exactly the following recipe menu, then end the turn and wait for the user's reply:

   ```
   1) CraftEngine — Assemble pistons + gears + circuits + canvas into an engine.
   2) CraftEngineTuned — Tuned engine — reaction-chamber + catalyst, deterministic and fast.
   pick recipe:
   ```

2. First check for exit words. If the reply (case-insensitive, trimmed) is `cancel`, `quit`, `exit`, `q`, or `nevermind`, output exactly `cancelled` and stop. Otherwise parse as an integer in the range 1..2. If invalid, output exactly `invalid choice` and stop.

3. Branch on the chosen recipe number:

   - **1** → `action_id="CraftEngine"`, inputs: 1 Pistons, 2 Gear, 2 Circuit, 1 Canvas, outputs: 1 Engine.
   - **2** → `action_id="CraftEngineTuned"`, inputs: 1 Pistons, 2 Gear, 2 Circuit, 1 Canvas, 1 Catalyst, outputs: 1 Engine.

4. For each input slot of the chosen recipe (looked up in step 3), in order:
   - Call `list_inventory`. Filter to live objects matching the slot's class.
   - If fewer than the slot's required count are available, output `no <class> available — run <producer>` and stop. `<producer>` is the bitcraft command that produces that class (e.g. `mine-iron` for `Iron`, `farm-water` for `Water`, `craft-flux` for `Flux`).
   - Output candidates and prompt:

     ```
     1) <file_name of first candidate>
     2) <file_name of second candidate>
     ...
     pick <class>:
     ```

   - If the slot's count is >1, prompt `pick <count> <class> (comma-separated, e.g. 1,2):` instead and parse as that many distinct integers.
   - Exit-word check (`cancel`/`quit`/`exit`/`q`/`nevermind` → `cancelled`).
   - Parse choice(s). Invalid → `invalid choice` and stop.
   - Append the chosen `file_path` value(s) to the running `input_object_paths` list, in order.

5. Call `run_action` with the chosen recipe's `action_id` and the accumulated `input_object_paths`.

6. On success, for each entry in the tool result's `outputs` array, output one line:

   `<class_name> → <output_path>`

7. On tool error, output the tool's error message verbatim, on one line. Stop.
