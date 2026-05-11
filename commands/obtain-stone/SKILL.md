---
name: bitcraft-obtain-stone
description: Bitcraft command — mine one Stone using a WoodPick or StonePick. Use when the user types "obtain-stone", "mine stone", "get stone", or asks bitcraft to produce a Stone.
---

# obtain-stone

Mine one `Stone` using a pick. The user picks which pick. The chosen pick loses durability.

Two actions back this command:
- `MineStoneWithWoodPick` — consumes a `WoodPick`.
- `MineStoneWithStonePick` — consumes a `StonePick`.

## Steps

1. Call `list_inventory`. Filter live objects for class `WoodPick` and class `StonePick`.
2. If both lists are empty: print `no pick available — run craft-wood-pick` and stop.
3. Print live picks together, labeled by class:
   ```
   W<n>) <wood pick file_name>
   S<n>) <stone pick file_name>
   ```
   Ask the user for one (`W<n>` or `S<n>`). One line.
4. If the chosen pick is a `WoodPick`: `action_id="MineStoneWithWoodPick"`. If `StonePick`: `action_id="MineStoneWithStonePick"`.
5. Call `run_action` with that `action_id` and `input_object_paths=[<chosen pick path>]`.
6. Report exactly one line:

   ```
   Stone → <output_path>
   ```

No commentary. Errors print verbatim and stop.

## Related

- `craft-wood-pick`, `craft-stone-pick` — produce the pick.
- `craft-stone-pick` — combine the new Stone + a Stick into a StonePick.
