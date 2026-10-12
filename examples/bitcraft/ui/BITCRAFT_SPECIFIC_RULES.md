# Bitcraft-specific rules in the bitcraft UI

The bitcraft UI is a client of the generic dobjd HTTP API. This file lists
every place where the UI behaves differently than it would for an arbitrary
Digital Objects store, because it knows something about the `bitcraft`
plugin that the API does not tell it.

Add an entry whenever a new rule of this kind goes in, and remove it when
the rule goes away. In `index.html`, the rules live between the
`Bitcraft-specific rules` markers at the top of the script; anything outside
that block should be plugin-agnostic.

## Scope

### Only bitcraft objects are listed

- **Where:** `PLUGIN_NAME`, applied in `load()` to the result of
  `GET /objects/unspent`.
- **Generic behavior:** list unspent objects from every installed plugin.
- **Why:** this is a bitcraft inventory; other plugins' objects (e.g.
  `craft-basics`, `craft-rocket`, `nanoverse`) are out of scope.

### Only bitcraft actions are offered in the Execute pane

- **Where:** `PLUGIN_NAME`, applied in `load()` to the result of
  `GET /actions`.
- **Generic behavior:** offer actions from every installed plugin. With no
  objects selected, that would also show other plugins' no-input actions.
- **Why:** same scope as the object list.

### Only bitcraft runs are restored on page load

- **Where:** `PLUGIN_NAME`, applied in `restoreRuns()` to the result of
  `GET /actions/runs?status=active`.
- **Generic behavior:** restore every run dobjd has in progress, whichever
  plugin's action it runs (runs can also be started by the CLI or an MCP
  agent).
- **Why:** same scope as the object list.

## Action input constraints

`GET /actions` reports only the classes an action takes as inputs. Some
bitcraft actions also constrain the values of those inputs' fields, so
matching by class alone offers actions that would fail. These rules are the
`ACTION_INPUT_RULES` entries; an action matching a rule's `actions` pattern
is offered only if the rule's `accepts` check passes for the selected
objects.

| Actions | Rule | Source in `plugin.rhai` |
| --- | --- | --- |
| `UseQuarry1` .. `UseQuarry6` | the selected `Quarry`'s `level` equals the number in the action name | `action.st_equal(quarry.level, N)` |
| `UseMiningDrill1` .. `UseMiningDrill6` | the selected `MiningDrill`'s `level` equals the number in the action name | `action.st_equal(miningdrill.level, N)` |
| `BucketOfWoodTake1`, `BucketOfStoneTake1`, `BucketOfIronTake1` | the selected bucket's `n` is at least 1, so an empty bucket offers no take | `action.st_gt_eq(woodbucket.n, 1)` (and the stone and iron buckets) |

- **Generic behavior:** offer every action whose input classes match the
  selection. A level-1 drill with 5 wood would show all six
  `UseMiningDrillN` actions.

### Known constraints not yet encoded

- `LevelUpQuarry1` .. `LevelUpQuarry5` and `LevelUpMiningDrill1` ..
  `LevelUpMiningDrill5` require each input bucket's `n` to equal a fixed
  amount per action (e.g. `LevelUpQuarry1`: wood bucket 40, stone bucket
  20). They do **not** check the quarry's or drill's current level, so any
  of them can level up a tool of any level, given the matching buckets.
- `UseWoodAxe`, `UseStoneAxe` and `UseIronAxe` require the axe's
  `durability` to be greater than 0.

## Cosmetic

- **Page title:** "Bitcraft Inventory".
- **Class icons:** `class-icons.json` maps each bitcraft class name to a
  16x16 PNG in `assets/` (paths relative to this folder), or to `null` for
  a class with no icon yet (every class currently has one). All three
  bucket classes share `bucket.png`, and every level of `Quarry` and
  `MiningDrill` uses the one icon for its class, since level is a field,
  not a class. The object
  list shows each object's icon in place of the page icon, on a 16x16
  rounded white tile with a light shadow; the same small icon sits to the
  left of every class name the Details pane shows (object summary lines,
  an action's selected inputs and expected outputs); and the Details pane
  for a single
  selected object draws it at 6x (96x96 CSS px) on the large page icon,
  centered across it and just below its folded corner (`CLASS_ICONS_URL`
  and `classIconPaths` in `index.html`). A class with no icon, or a
  missing or unreadable file, keeps the plain page icon. Keep the file's
  keys in step with the classes in `../manifest.toml`. Generic behavior:
  plain page icons only, since the API reports no icons.

## Action icons

Each action gets a 32x32 app-style graphic, chosen by its category (tool
interaction, material creation or raw resource gathering), in the Available
actions list and in the Details pane for a selected action or job. The
rules, and how to add to them, are in `ACTION_ICONS.md`; the data is
`action-icons.json`. Generic behavior: no action graphics.
