---
name: bitcraft-craft-circuit
description: Craft Circuit (5 recipe variants).
---

# craft-circuit

## Output rules

- Plain text only. No markdown bold, italics, bullets, code fences, or headers in user-facing output.
- No preamble. No closing summary. No suggestions. No commentary.
- Do not mention any other command, skill, or capability.

## Steps

1. Output exactly the following recipe menu, then end the turn and wait for the user's reply:

   ```
   1) CraftCircuit — Solder 2 wire + 1 steel into a circuit (PoW + VDF).
   2) CraftCircuitSoldered — Soldering-iron variant — eliminates PoW, deterministic.
   3) CraftCircuitFabbed — Circuit-fab variant — bulk production at the station.
   4) CraftCircuitFlash — Flash circuit (PoW only) — gamble for throughput.
   5) CraftCircuitCrude — Crude circuit — recovers a wire, slower.
   pick recipe:
   ```

2. First check for exit words. If the reply (case-insensitive, trimmed) is `cancel`, `quit`, `exit`, `q`, or `nevermind`, output exactly `cancelled` and stop. Otherwise parse as an integer in the range 1..5. If invalid, output exactly `invalid choice` and stop.

3. Branch on the chosen recipe number:

   - **1** → `action_id="CraftCircuit"`, inputs: 2 Wire, 1 Steel, outputs: 1 Circuit.
   - **2** → `action_id="CraftCircuitSoldered"`, inputs: 2 Wire, 1 Steel, outputs: 1 Circuit.
   - **3** → `action_id="CraftCircuitFabbed"`, inputs: 4 Wire, 1 Steel, outputs: 2 Circuit.
   - **4** → `action_id="CraftCircuitFlash"`, inputs: 2 Wire, 1 Steel, outputs: 1 Circuit.
   - **5** → `action_id="CraftCircuitCrude"`, inputs: 2 Wire, 1 Steel, outputs: 1 Circuit, 1 Wire.

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
