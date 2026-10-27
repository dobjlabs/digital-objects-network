# Presentation packs

A presentation pack gives a plugin's classes and actions what an app needs
to show them: an icon, a one-line summary and a longer description. Packs
belong to apps. dobjd and the `.pexe` format never read them, so a plugin
works the same whether its pack exists, is incomplete or is wrong; an app
just shows less.

Bitcraft uses one for the `bitcraft` plugin
(`examples/bitcraft/presentation/`). The same pattern works for any app and
for any plugin, including extensions of bitcraft.

## The three pieces

| Piece | What it is | How many | Where |
| --- | --- | --- | --- |
| `pack.json` and its assets | One plugin's presentation data | One per plugin | Beside the plugin's source, then copied into each app that uses it |
| `index.json` | The packs an app ships | One per app | In the app's UI, generated when the packs are copied in |
| `presentation.js` | Reads the index and packs, and answers lookups | One file, copied into each app | In the app's UI |

An app's UI holds the packs it uses under one folder:

```
ui/
  presentation.js
  presentation/
    index.json            { "packs": ["bitcraft/pack.json", "my-extension/pack.json"] }
    bitcraft/
      pack.json
      assets/...
    my-extension/
      pack.json
      assets/...
```

## pack.json

```json
{
  "format": 1,
  "plugin": "bitcraft",
  "writtenFor": { "moduleHash": "8fdd..." },
  "classes": {
    "Wood": {
      "icon": "assets/classes/wood.png",
      "summary": "A basic building material.",
      "description": "Gathered with a pocketknife or chopped with an axe..."
    }
  },
  "actions": {
    "GatherWood": {
      "icon": "assets/actions/GatherWood.svg",
      "summary": "Gather 1 wood with your pocketknife.",
      "description": "Your computer does a little work first..."
    }
  }
}
```

| Field | Meaning |
| --- | --- |
| `format` | `1`. |
| `plugin` | The plugin's name, as in its manifest. Lookups are by qualified name (`plugin::Name`), so packs for different plugins never collide. |
| `writtenFor.moduleHash` | Optional. The plugin version the pack was written for. A hint for tools that warn about stale packs; never enforced. |
| `classes`, `actions` | Entries keyed by bare class or action name. List built-in actions (such as `Rekey<Class>`) like any other action. |
| `icon` | A path relative to `pack.json` to a **finished image** (PNG or SVG) that the app displays as is. Compose badges, tiles and so on when generating the pack, not in the app. |
| `summary` | One line, for lists and headings. |
| `description` | A few sentences, for details panes. |

Every field and entry is optional.

## presentation.js

```js
await Presentation.load();            // reads presentation/index.json and every pack it lists
Presentation.classInfo({ pluginName: "bitcraft", name: "Wood" });
Presentation.actionInfo({ pluginName: "bitcraft", name: "GatherWood" });
// -> { icon: "https://.../presentation/bitcraft/assets/actions/GatherWood.svg",
//      summary: "...", description: "..." }   (each null when unknown)
```

It never fails the page. A missing or unreadable index, a pack that is
missing, not JSON or has no `plugin`, an entry that is missing, and a field
that is not a non-empty string all come back as `null`. The app decides
what to show instead: Bitcraft draws a plain page icon for a class with no
icon, leaves an action's icon slot empty, and falls back to the manifest's
`description` from dobjd's API (skipping the `"-"` placeholder).

The file has no dependencies and no build step. It resolves asset URLs
against the index's location, so the folder can live anywhere the page can
fetch.

## Adding a pack to an app

1. Keep the pack beside its plugin's source (`<plugin>/presentation/`).
2. Copy its `pack.json` and `assets/` into the app's `presentation/<plugin>/`
   and list it in `presentation/index.json`. Bitcraft's
   `apps/bitcraft/scripts/sync_presentation.py` does both; add the pack's
   folder to its `PACKS` list.
3. Nothing else changes: no app code, no daemon, no plugin.

## Starting a new app

Copy `presentation.js`, write a step like `sync_presentation.py` that copies
your packs and writes the index, and call `Presentation.load()` before
drawing classes or actions.

## Writing a pack

Hand-write `pack.json`, or generate it. Bitcraft generates its pack with
`examples/bitcraft/presentation/generate.py`, which draws every action icon
from class art and a badge, takes the summaries and descriptions from
hand-written Markdown (`copy/classes.md` and `copy/actions.md`, where one
entry can cover a family of similar actions), and checks that every class
and action in the manifest has an entry. Generating keeps families of
similar actions consistent without putting any rules in the app.
