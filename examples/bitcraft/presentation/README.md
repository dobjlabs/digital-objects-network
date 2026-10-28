# Bitcraft presentation pack

> **Bitcraft-specific, and read only by apps.** This folder gives every
> bitcraft class and action an icon, a one-line summary and a longer
> description. dobjd and the `.pexe` never read it: the plugin works the
> same whether this folder exists, is incomplete or is wrong. The format is
> described in `../../../apps/bitcraft/PRESENTATION_PACKS.md`.

| Path | What it is |
| --- | --- |
| `pack.json` | The pack. **Generated; do not edit.** |
| `copy/classes.md`, `copy/actions.md` | Every summary and description. **Hand-written; edit these.** See [Copy](#copy). |
| `assets/classes/*.png` | Class art, 16x16 RGBA pixel art. Hand-drawn; edit these directly. Also the art drawn on action icons. |
| `assets/actions/*.svg` | One finished icon per action. **Generated.** |
| `source/refresh.svg` | Art drawn on the rekey icons only. |
| `generate.py` | Writes `pack.json` and `assets/actions/` from the copy, the class art and the icon rules below. |
| `test_generate.py` | Tests for reading the copy, and that `pack.json` and the icons are up to date. |

## Regenerating

After editing the copy or the art, or changing a recipe in
`../src/main.rs` or the plugin's classes or actions:

```sh
just bitcraft-presentation
```

That runs `generate.py`, then copies the pack into the Bitcraft UI
(`apps/bitcraft/scripts/sync_presentation.py`). `just bitcraft-prepare`
also copies it. Commit the copy, `pack.json` and `assets/actions/`
together.

`generate.py` reads the class and action names from `../manifest.toml`,
adds the built-in `Rekey<Class>` action for each class, and writes nothing
if any of them has no art, icon rule or copy, or if the copy has a problem,
listing each with its file and line. `generate.py --check` writes nothing
and fails if `pack.json` or an icon is not what it would write; CI runs it,
with `test_generate.py`, so edited copy that was not regenerated is caught.

The copy quotes recipe amounts (yields, level-up bucket sizes and so on),
and nothing checks prose against the recipes, so update it by hand when
`../src/main.rs` changes.

## Copy

Every summary and description is hand-written in `copy/classes.md` (one
entry per class) and `copy/actions.md` (one per action, or per family of
actions). The copy is written and maintained by Brian; agents do not edit
it (see `CLAUDE.md`). Each entry is a `##` heading, then paragraphs:

```markdown
## Wood

A basic building material.

Gathered with a pocketknife or chopped with an axe. Goes into sticks,
axes, quarries and mining drills.
```

- **Heading:** the class or action name, exactly as in the manifest.
  Anything before the first heading (a title, notes) is ignored.
- **Summary:** the first paragraph, one line in the apps. Lines of a
  paragraph are joined with spaces, so wrap them however you like.
- **Description:** every paragraph after the first, kept as separate
  paragraphs. Optional.
- No other headings are allowed after the first entry, and a name may have
  only one entry.

### Families

A heading with `{placeholders}` is a family: it covers every name it fits,
and its text fills in what each placeholder matched. An entry for a single
name overrides its family, so one member can say something different.

```markdown
## LevelUpQuarry{level}

Raise a level {level} quarry to level {level + 1}.

Spends a bucket holding exactly {20 * 2 ** level} wood.
```

In the text, a field in braces can be:

| Field | Gives, for `LevelUpQuarry3`, `BucketOfWoodAdd10` or `RekeyBucketOfWood` |
| --- | --- |
| `{level}` | what the placeholder matched: `3` |
| `{resource\|lower}` | the match in lower case: `wood` |
| `{class\|words}` | a CamelCase match as lower-case words: `bucket of wood` |
| `{level + 1}`, `{20 * 2 ** level}` | whole-number arithmetic on placeholders, with `+ - * ** //`: `4`, `160` |
| `{{`, `}}` | a literal brace |

A name two families fit is an error: give it an entry of its own. So is an
entry or family that matches nothing in the manifest, which catches copy
left behind by a renamed or removed action.

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
buckets, barrels, axes, quarries, drills and pumpjacks.

- **Main icon:** the tool being interacted with (pocketknife, bucket, barrel, axe,
  quarry, drill or pumpjack).
- **Badge:**

| Interaction | Badge | Actions |
| --- | --- | --- |
| Creating the tool | green plus | `CraftPocketknife`, `CraftBucketOf{Wood,Stone,Iron,Bits,Coal,Quartz}`, `CraftBarrelOfOil`, `CraftWoodAxe`, `CraftStoneAxe`, `CraftIronAxe`, `BuildQuarry`, `BuildMiningDrill`, `BuildPumpjack` |
| Adding to a bucket | green up chevron (^) | `BucketOf{Wood,Stone,Iron,Bits,Coal,Quartz}Add{1,5,10,50}`, and `BitsFrom{Quartz,Amethyst,Sapphire,Emerald,Ruby,Diamond}`, which add a gem's bits to a bucket of bits |
| Taking from a bucket | red down chevron (v) | `BucketOf{Wood,Stone,Iron,Bits,Coal,Quartz}Take1` |
| Using an axe to produce wood | none | `UseWoodAxe`, `UseStoneAxe`, `UseIronAxe` |
| Using a quarry or drill to produce stone, iron, coal or geodes, or a pumpjack to fill a barrel of oil | none | `UseQuarry1`-`10`, `UseMiningDrill1`-`10`, `MineCoal1`-`10`, `MineGem1`-`10`, `PumpOil` |
| Upgrading a quarry, drill or pumpjack | green up chevron (^) | `LevelUpQuarry1`-`9`, `LevelUpMiningDrill1`-`9`, `LevelUpPumpjack1`-`9` |

Using a tool shows nothing in either corner. Every level of a quarry,
drill or pumpjack uses the one icon for its class, since level is a field, not a class.

### Material creation

Turning resources into intermediate materials, or a geode into its gem.

- **Main icon:** the material created.
- **Badge:** green plus. All current actions in this category create
  something.

| Actions | Main icon |
| --- | --- |
| `CraftStick` | Stick |
| `BitsFromWood`, `BitsFromStone`, `BitsFromIron` | Bit |
| `CrackGeode{Quartz,Amethyst,Sapphire,Emerald,Ruby,Diamond}` | the gem cracked out |

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

## Where the icon rules live

`ACTION_ART_RULES` in `generate.py` is an ordered list of rules; the first
rule whose pattern fully matches an action's name decides its icon's art
and badge. `BUILTIN_ART_RULES` does the same for the built-in actions,
matched only for a class the manifest declares. Each rule returns the art
(a class name, whose art in `CLASS_ART_FILES` is used, or a path to other
art) and the badge (`plus`, `up`, `down` or none).

## Adding actions or classes

When `../plugin.rhai` gains actions, decide which category each belongs
to, then add or extend a rule in `generate.py`, update the tables above,
write its copy in `copy/actions.md`, and regenerate. A new class also needs
its art in `assets/classes/`, an entry in `CLASS_ART_FILES`, and its copy in
`copy/classes.md`.

Bucket classes and their actions are named `BucketOf<Resource>`, except the
bucket of bits, which is `BucketOfBits` (with `CraftBucketOfBits`,
`BucketOfBitsAdd<n>` and `BucketOfBitsTake1`). Patterns that list bucket
resources must spell it `Bits`; copy families like `BucketOf{resource}` read
it as "bits".
