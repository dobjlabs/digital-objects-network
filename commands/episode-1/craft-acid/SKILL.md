---
name: bitcraft-craft-acid
description: Craft Acid (3 recipe variants).
---

# craft-acid

## Output rules

- Plain text only. No markdown bold, italics, bullets, code fences, or headers in user-facing output.
- No preamble. No closing summary. No suggestions. No commentary.
- Do not mention any other command, skill, or capability.

## Steps

1. Output exactly the following recipe menu, then end the turn and wait for the user's reply:

   ```
   1) CraftAcid — PoW-mediated acid production: sulfur + water → 2 acid.
   2) CraftAcidFlash — Flash acid (PoW only): sulfur + water → 1 acid, faster mean.
   3) CraftAcidCrude — Crude acid (both): sulfur + water → 3 acid, throughput-tilted.
   pick recipe:
   ```

2. First check for exit words. If the reply (case-insensitive, trimmed) is `cancel`, `quit`, `exit`, `q`, or `nevermind`, output exactly `cancelled` and stop. Otherwise parse as an integer in the range 1..3. If invalid, output exactly `invalid choice` and stop.

3. Branch on the chosen recipe number:

   - **1** → `action_id="CraftAcid"`, inputs: 1 Sulfur, 1 Water, outputs: 2 Acid.
   - **2** → `action_id="CraftAcidFlash"`, inputs: 1 Sulfur, 1 Water, outputs: 1 Acid.
   - **3** → `action_id="CraftAcidCrude"`, inputs: 1 Sulfur, 1 Water, outputs: 3 Acid.

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
