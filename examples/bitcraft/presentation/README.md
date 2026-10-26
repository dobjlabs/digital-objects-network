# Bitcraft presentation pack

> **Bitcraft-specific, and read only by apps.** This folder gives every
> bitcraft class and action an icon, a one-line summary and a longer
> description. dobjd and the `.pexe` never read it: the plugin works the
> same whether this folder exists, is incomplete or is wrong. The format is
> described in `../../../apps/bitcraft/PRESENTATION_PACKS.md`.

| Path | What it is |
| --- | --- |
| `pack.json` | The pack. **Generated; do not edit.** |
| `assets/classes/*.png` | Class art, 16x16 RGBA pixel art. Hand-drawn; edit these directly. Also the art drawn on action icons. |
| `assets/actions/*.svg` | One finished icon per action. **Generated.** |
| `source/refresh.svg` | Art drawn on the rekey icons only. |
| `generate.py` | Writes `pack.json` and `assets/actions/`. Holds the class art mapping, the icon rules below and all the text. |

## Regenerating

After changing a recipe in `../src/main.rs`, adding a class or action,
editing art, or changing text in `generate.py`:

```sh
just bitcraft-presentation
```

That runs `generate.py`, then copies the pack into the Bitcraft UI
(`apps/bitcraft/scripts/sync_presentation.py`). `just bitcraft-prepare`
also copies it. `generate.py` reads the class and action names from
`../manifest.toml`, adds the built-in `Rekey<Class>` action for each class,
and writes nothing if any of them has no art or text, listing what is
missing. Commit `pack.json` and `assets/actions/` with the change.

The text quotes recipe amounts (yields, level-up bucket sizes and so on),
so keep `generate.py` in step with `../src/main.rs`.

## Action icons

Each action icon is a 32x32 SVG styled like a macOS app icon: a white
rounded tile (grid units 2-30, about 22% corner radius) with a faint edge
and a drop shadow, the art centered on it at 16 units (grid units 8-24),
and an optional badge in the top-right corner. Pixel art is drawn as
unit squares rather than an embedded bitmap, so the icon stays sharp at
any size in any browser. Apps show the icons at 16x16 (context menu, jobs
list), 32x32 (Recipe Info list) and 128x128 (details panes).

## Categories

Every action belongs to one of four categories, and each category decides
the main icon and the badge differently.

### Tool interaction

Creating, filling, emptying, using and upgrading tools: pocketknives,
buckets, axes, quarries and drills.

- **Main icon:** the tool being interacted with (pocketknife, bucket, axe,
  quarry or drill).
- **Badge:**

| Interaction | Badge | Actions |
| --- | --- | --- |
| Creating the tool | green plus | `CraftPocketknife`, `CraftBucketOfWood`, `CraftBucketOfStone`, `CraftBucketOfIron`, `CraftWoodAxe`, `CraftStoneAxe`, `CraftIronAxe`, `BuildQuarry`, `BuildMiningDrill` |
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

Gathering raw resources with a pocketknife, which is kept (proof-of-work).

- **Main icon:** the resource gathered.
- **Badge:** green plus. All current actions in this category create
  something.

| Actions | Main icon |
| --- | --- |
| `GatherWood`, `GatherStone`, `GatherIron` | Wood, Stone, Iron |

### Rekey

The built-in `Rekey<Class>` action every class gets, which replaces an
object's key and keeps its other fields. These come from the SDK, not
`../plugin.rhai`, so one rule covers every class.

- **Main icon:** a blue refresh symbol (`source/refresh.svg`), the same for
  every class.
- **Badge:** none.

## Badges

Drawn in the top-right corner of the 32x32 grid, overhanging the tile like
an app's notification badge, with a white outline so they read over any
art.

| Badge | Look | Meaning |
| --- | --- | --- |
| `plus` | green + | creates something |
| `up` | green ^ | adds to or upgrades a tool |
| `down` | red v | takes from a tool |

## Where the rules live

`ACTION_RULES` in `generate.py` is an ordered list of rules; the first rule
whose pattern fully matches an action's name decides its art, badge and
text. `BUILTIN_RULES` does the same for the built-in actions, matched only
for a class the manifest declares. Each rule returns the art (a class name,
whose art in `CLASSES` is used, or a path to other art), the badge (`plus`,
`up`, `down` or none) and the summary and description, filled in from the
pattern's capture groups, so one rule covers a family of actions.

## Adding actions or classes

When `../plugin.rhai` gains actions, decide which category each belongs
to, then add or extend a rule in `generate.py`, update the tables above,
and regenerate. A new class also needs its art in `assets/classes/` and an
entry in `CLASSES`.
