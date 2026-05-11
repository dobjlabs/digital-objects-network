---
name: bitcraft-craft-wood
description: Bitcraft command — refine one Log into a Wood object via CraftWood. Use when the user types "craft-wood", "craft wood", "make wood", or asks bitcraft to refine a Log.
---

# craft-wood

Refine one `Log` into one `Wood`. The user picks the Log.

## Steps

1. Call `list_inventory`. Filter for live `Log` objects.
2. If zero live Logs: print `no Log available — run obtain-log` and stop.
3. Print each live Log as `<n>) <file_name>` and ask the user which `<n>` to use. One line only.
4. Call `run_action` with `action_id="CraftWood"` and `input_object_paths=[<chosen path>]`.
5. Report exactly one line:

   ```
   Wood → <output_path>
   ```

No commentary. Errors print verbatim and stop.

## Related

- `obtain-log` — produce a Log first.
- `craft-sticks` — turn this Wood into Sticks.
- `craft-wood-pick` — combine Wood + Stick into a WoodPick.
