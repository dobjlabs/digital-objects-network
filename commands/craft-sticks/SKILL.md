---
name: bitcraft-craft-sticks
description: Bitcraft command — split one Wood into two Sticks via CraftSticks. Use when the user types "craft-sticks", "craft sticks", "make sticks", or asks bitcraft to produce Sticks.
---

# craft-sticks

Split one `Wood` into two `Stick` objects. The user picks the Wood.

## Steps

1. Call `list_inventory`. Filter for live `Wood` objects.
2. If zero live Wood: print `no Wood available — run craft-wood` and stop.
3. Print each live Wood as `<n>) <file_name>` and ask the user which `<n>` to use. One line only.
4. Call `run_action` with `action_id="CraftSticks"` and `input_object_paths=[<chosen path>]`.
5. Report exactly two lines:

   ```
   Stick → <output_path_1>
   Stick → <output_path_2>
   ```

No commentary. Errors print verbatim and stop.

## Related

- `craft-wood` — produce Wood first.
- `craft-wood-pick` — combine Wood + Stick into a WoodPick.
- `craft-stone-pick` — combine Stone + Stick into a StonePick.
