---
name: bitcraft-craft-wood-pick
description: Bitcraft command — combine one Wood and one Stick into a WoodPick via CraftWoodPick. Use when the user types "craft-wood-pick", "craft wood pick", "make a wood pick", or asks bitcraft to craft a WoodPick.
---

# craft-wood-pick

Combine one `Wood` and one `Stick` into one `WoodPick`. The user picks each input.

## Steps

1. Call `list_inventory`. Filter live objects for class `Wood` and class `Stick`.
2. If zero live Wood: print `no Wood available — run craft-wood` and stop.
3. If zero live Sticks: print `no Stick available — run craft-sticks` and stop.
4. Print live Wood as `W<n>) <file_name>` and live Sticks as `S<n>) <file_name>`. Ask the user for one of each (`W<n>` and `S<n>`). One prompt, two answers.
5. Call `run_action` with `action_id="CraftWoodPick"` and `input_object_paths=[<chosen Wood path>, <chosen Stick path>]`.
6. Report exactly one line:

   ```
   WoodPick → <output_path>
   ```

No commentary. Errors print verbatim and stop.

## Related

- `craft-wood`, `craft-sticks` — prerequisites.
- `obtain-stone` — use the WoodPick to mine Stone.
