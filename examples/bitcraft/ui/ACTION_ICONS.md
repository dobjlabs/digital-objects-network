# Action icons

> **Bitcraft-specific.** Everything in this file describes how the bitcraft
> UI draws icons for the `bitcraft` plugin's actions. The dobjd API reports
> no icons, and a generic Digital Objects UI would draw none. It is one of
> the bitcraft-specific behaviors listed in `BITCRAFT_SPECIFIC_RULES.md`.

Each action gets a 32x32 graphic, styled like a macOS app icon: a plain
white rounded tile (grid units 2-30, about 22% corner radius) with a faint
edge and a drop shadow, the main class art centered on it (grid units
8-24), and an optional badge in the top-right corner. It is shown in three
places:

- the Available actions list, at 32x32, left of the action name;
- the Details pane when an action is selected, at 4x, replacing the blank
  page;
- the Details pane when a job is selected, at 4x, for the job's action.

## Categories

Every action belongs to one of three categories, and each category decides
the main icon and the badge differently.

### Tool interaction

Creating, filling, emptying, using and upgrading tools: buckets, axes,
quarries and drills.

- **Main icon:** the tool being interacted with (bucket, axe, quarry or
  drill).
- **Badge:**

| Interaction | Badge | Actions |
| --- | --- | --- |
| Creating the tool | green plus | `CraftBucketOfWood`, `CraftBucketOfStone`, `CraftBucketOfIron`, `CraftWoodAxe`, `CraftStoneAxe`, `CraftIronAxe`, `BuildQuarry`, `BuildMiningDrill` |
| Adding to a bucket | green up chevron (^) | `BucketOf{Wood,Stone,Iron}Add{1,5,10,50}` |
| Taking from a bucket | red down chevron (v) | `BucketOf{Wood,Stone,Iron}Take1` |
| Using an axe to produce wood | none | `UseWoodAxe`, `UseStoneAxe`, `UseIronAxe` |
| Using a quarry or drill to produce stone or iron | none | `UseQuarry1`-`6`, `UseMiningDrill1`-`6` |
| Upgrading a quarry or drill | green up chevron (^) | `LevelUpQuarry1`-`5`, `LevelUpMiningDrill1`-`5` |

Using a tool shows nothing in either corner. Every level of a quarry or
drill uses the one icon for its class, since level is a field, not a class.

### Material creation

Turning resources into intermediate materials.

- **Main icon:** the material created.
- **Badge:** green plus. All current actions in this category create
  something.

| Actions | Main icon |
| --- | --- |
| `CraftStick` | Stick |
| `BitsFromWood`, `BitsFromStone`, `BitsFromIron` | Bit |

### Raw resource gathering

Gathering raw resources from nothing (proof-of-work).

- **Main icon:** the resource gathered.
- **Badge:** green plus. All current actions in this category create
  something.

| Actions | Main icon |
| --- | --- |
| `GatherWood`, `GatherStone`, `GatherIron` | Wood, Stone, Iron |

## Badges

Drawn in the top-right corner of the 32x32 grid, overhanging the tile like
an app's notification badge, with a white outline so they read over any
art.

| Badge | Look | Meaning |
| --- | --- | --- |
| `plus` | green + | creates something |
| `up` | green ^ | adds to or upgrades a tool |
| `down` | red v | takes from a tool |

## How the rules are stored

`action-icons.json` holds an ordered list of `rules`; the first rule whose
`actions` pattern matches the action's name decides its graphic
(`ACTION_ICONS_URL`, `actionIconSpec` and `actionIcon` in `index.html`).
Each rule has:

| Field | Meaning |
| --- | --- |
| `category` | `tool interaction`, `material creation` or `raw resource gathering`; for people, not read by the page |
| `description` | For people; not read by the page |
| `actions` | Regular expression matched against the bare action name (e.g. `GatherWood`) |
| `icon` | Class whose art is centered on the tile |
| `badge` | Optional; `plus`, `up` or `down` (see Badges). Omit for no badge |
| `cornerIcon` | Optional; class whose icon is drawn at 8x8 in the tile's bottom-right corner (grid units 20-28). No current rule uses it |

`icon` and `cornerIcon` are class names looked up in `class-icons.json`,
and may use `$1`, `$2`, ... for the pattern's capture groups, so one rule
covers a family of actions. An action that matches no rule, or whose `icon`
class has no art, gets an empty slot in the list and the blank page in the
Details pane.

All 55 current bitcraft actions match a rule: 48 tool interaction, 4
material creation, 3 raw resource gathering.

## Adding actions

When `../plugin.rhai` gains actions, decide which category each belongs
to, then add or extend a rule in `action-icons.json` and update the tables
above in the same change.
