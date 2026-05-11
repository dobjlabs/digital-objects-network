---
name: bitcraft-craft-stone-pick
description: Bitcraft command — combine one Stone and one Stick into a StonePick via CraftStonePick. Use when the user types "craft-stone-pick", "craft stone pick", "make a stone pick", or asks bitcraft to craft a StonePick.
---

# craft-stone-pick

Combine one `Stone` and one `Stick` into one `StonePick`. The user picks each input.

## Steps

1. Call `list_inventory`. Filter live objects for class `Stone` and class `Stick`.
2. If zero live Stone: print `no Stone available — run obtain-stone` and stop.
3. If zero live Sticks: print `no Stick available — run craft-sticks` and stop.
4. Print live Stones as `St<n>) <file_name>` and live Sticks as `Sk<n>) <file_name>`. Ask the user for one of each. One prompt, two answers.
5. Call `run_action` with `action_id="CraftStonePick"` and `input_object_paths=[<chosen Stone path>, <chosen Stick path>]`.
6. Report exactly one line:

   ```
   StonePick → <output_path>
   ```

No commentary. Errors print verbatim and stop.

## Related

- `obtain-stone`, `craft-sticks` — prerequisites.
- `obtain-stone` — use the StonePick to mine more Stone.
