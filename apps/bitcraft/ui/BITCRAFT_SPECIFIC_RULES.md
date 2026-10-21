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

### Only bitcraft actions are offered in the context menu

- **Where:** `PLUGIN_NAME`, applied in `load()` to the result of
  `GET /actions`.
- **Generic behavior:** offer actions from every installed plugin in the
  objects' context menu. A right-click on empty space, which offers the
  no-input actions, would also show other plugins' no-input actions.
- **Why:** same scope as the object list.

### Only bitcraft actions are listed in the Recipe Info tab

- **Where:** `PLUGIN_NAME`, applied in `load()` to the result of
  `GET /actions`; the Recipe Info tab lists the same filtered actions the
  context menu offers from.
- **Generic behavior:** list every installed plugin's actions.
- **Why:** same scope as the object list.

### Only bitcraft runs are restored on page load

- **Where:** `PLUGIN_NAME`, applied in `restoreRuns()` to the result of
  `GET /actions/runs?status=active`.
- **Generic behavior:** restore every run dobjd has in progress, whichever
  plugin's action it runs (runs can also be started by the CLI or an MCP
  agent).
- **Why:** same scope as the object list.

## Startup pocketknife

- **Where:** `ensureStartupPocketknife()`, called by `load()` after active
  runs have been restored successfully.
- **What:** if no live `bitcraft::Pocketknife` exists, start
  `bitcraft::CraftPocketknife` with no inputs. Live knives, including knives
  held by a run, count as existing. An active crafting run or another active
  action that outputs a pocketknife suppresses creation. Re-read inventory
  after restoring runs so a run that finished during startup does not cause
  another knife to be crafted.
- **Stale knives:** with no live knife and no such active run, a pending or
  unknown knife is one whose commit failed. The inventory re-read has already
  checked it against the synchronizer, so it is moved aside with
  `POST /objects/{file_name}/discard_unsafe`, which keeps none of dobjd's
  object-state guarantees, and a new knife is crafted. This
  happens on "Try again" after a failed commit and on reopening the app.
  Discarding first requires the synchronizer to answer `GET /state-root`,
  because the inventory falls back to local state when it is unreachable;
  otherwise setup shows an error and keeps the knife.
- **Startup screen:** keep inventory hidden and inert until a live
  pocketknife is listed. Show whether Bitcraft is creating the knife or
  waiting for network confirmation. Existing live knives, including held
  knives, open the inventory directly. The desktop's cryptography startup screen
  has the same layout, so initial preparation leads into pocketknife setup.
  Both screens use a simple centered layout with a progress bar and status
  text. The bar counts three completed milestones: cryptography ready,
  pocketknife proof generated, and network confirmation. Preparation starts
  at zero in the desktop shell; successful daemon requests complete the
  first milestone. A committing run, a completed proof event, or an existing
  knife that has not been discarded completes the second; a live knife completes the third. Progress
  stays at the last completed milestone during errors; retry starts a fresh
  check. Opening inventory after an error does not mark setup complete.
  Show the current step number and describe the work in progress in the
  heading, rather than labeling the bar with the last completed milestone.
  Errors replace the activity description while preserving the bar's value.
  Steps do not represent time remaining. Keep both screens' appearance in
  step when changing either.
- **Jobs:** automatic startup runs are tracked but omitted from Jobs.
  Remember their run ID in session storage so reloading the page keeps
  them hidden. Manual pocketknife jobs, including restored jobs without
  that marker, remain visible. The Jobs pane is hidden when no visible
  jobs exist; starting a manual job reveals it.
- **Failures:** show the cause with "Try again", "Copy error", and "Open inventory"
  controls if setup fails, active runs cannot be read, the synchronizer
  cannot be reached before discarding a stale knife, or the recipe is
  unavailable. Retry re-reads active jobs and inventory before discarding
  or creating anything. Opening inventory dismisses
  setup for this page session.
  When a progress read gets no HTTP response at all, the daemon has
  stopped, so the error says the background service stopped responding,
  suggests quitting and reopening Bitcraft, and names `dobjd.log` in the
  data directory (the parent of the objects directory).
  "Copy error" copies the full displayed error, including line breaks, and
  reports success or failure. The error text can also be selected manually.
  A failed run is never retried automatically while the page is open;
  reopening the app retries after discarding its stale knife. Run polling backs up SSE so a
  quiet terminal transition or an unavailable event stream cannot leave
  setup waiting forever; inventory is polled after success until the knife
  is live. Inventory refreshes alone do not trigger creation.
- **Generic behavior:** loading the UI never starts actions or discards objects automatically.

## Action input constraints

`GET /actions` reports only the classes an action takes as inputs. Some
bitcraft actions also constrain the values of those inputs' fields, so
matching by class alone offers actions that would fail. These rules are the
`ACTION_INPUT_RULES` entries; an action matching a rule's `actions` pattern
is offered in the context menu only if the rule's `accepts` check passes for
the selected objects.

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
  and a Recipe Info action's inputs and outputs); and the Details pane
  for a single
  selected object draws it at 6x (96x96 CSS px) on the large page icon,
  centered across it and just below its folded corner (`CLASS_ICONS_URL`
  and `classIconPaths` in `index.html`). A class with no icon, or a
  missing or unreadable file, keeps the plain page icon. Keep the file's
  keys in step with the classes in `../../../examples/bitcraft/manifest.toml`. Generic behavior:
  plain page icons only, since the API reports no icons.

## Object list footer and status bar

A one-line footer under the object list (the file count and "Total Bit
Value: 31", separated by a middle dot, like Finder's item count), and the status bar along the bottom of the
page. Two of their entries know about bitcraft:

- **Bitcraft version** (status bar): `BITCRAFT_VERSION` ("Alpha 1.0"), a
  fixed string in the page; dobjd and the plugin manifest report no game
  version.
- **Total Bit Value** (footer): the summed value of the live objects in
  the list (pending ones are left out); see below.
- **Generic behavior:** no version or Total Bit Value. The other entries
  are generic: the synchronizer and relayer statuses and addresses in the
  status bar (the page probes each service's `/healthz` at the URLs
  `GET /settings` reports, every 10 seconds), and the footer's count of
  files: every unspent object in the objects folder, pending ones
  included, whatever their plugin, plus the welcome guide when it is
  listed. Unlike the Total Bit Value, it counts pending objects.

### Total Bit Value

An object's Total Bit Value is the raw resources spent to make it, each
counted at the rate its `BitsFrom*` recipe pays: Wood 1, Stone 2, Iron 5.
A Bit is worth 1. The values are worked out by hand from `../../../examples/bitcraft/plugin.rhai`
and hardcoded (`TOTAL_BIT_VALUES`, `BUCKET_CONTENT_BIT_VALUES`,
`LEVELED_TOTAL_BIT_VALUES` in `index.html`). Recompute them whenever a
recipe's inputs or outputs change.

| Class | Total Bit Value | Made from |
| --- | --- | --- |
| Bit | 1 | |
| Wood, Stone, Iron | 1, 2, 5 | |
| Pocketknife | 0 | nothing (`CraftPocketknife` takes no inputs) |
| Stick | 0.5 | 1 Wood makes 2 Sticks |
| WoodAxe | 2 | 1 Wood + 2 Sticks |
| StoneAxe | 4 | WoodAxe + 1 Stone |
| IronAxe | 9 | StoneAxe + 1 Iron |
| BucketOfWood / Stone / Iron | 5 + n x 1 / 2 / 5 | 1 Iron, plus the `n` resources it holds |
| Quarry, by level 1-6 | 40, 130, 300, 630, 1280, 2570 | 20 Wood + 10 Stone, plus each level-up's buckets |
| MiningDrill, by level 1-6 | 135, 420, 975, 2070, 4245, 8580 | 10 Wood + 50 Stone + 5 Iron, plus each level-up's buckets |

A level-up spends full buckets, each worth 1 Iron plus its contents:
`LevelUpQuarryK` adds 10 + 40 x 2^K, and `LevelUpMiningDrillK` adds
15 + 135 x 2^K. Wear and rekeying do not change the value: an axe is worth
what it cost to make whatever its durability, and a `Rekey<Class>` action
leaves every value field alone. A total that is not whole shows one
decimal place.

## Welcome guide

- **Where:** `WELCOME_FILE`, loaded in `load()`; the text is
  `BITCRAFT_WELCOME.md` in this folder.
- **What:** a getting-started guide pinned as the first row of the object
  list, with a document icon. It is selected like any object (click,
  Cmd-click, Shift-click ranges). Selected alone, the Details pane renders
  its Markdown (headings, paragraphs, lists, pipe tables, bold and inline
  code only). Selected with objects, the Details pane counts it separately
  ("2 objects and 1 file") and lists it with its size and a "View raw"
  link but no status. Any selection that includes it gets only "Open" or
  "Open all" in the context menu, since it is no action's input.
- **Not from the objects folder:** the row is served next to the page, so
  it shows whether or not a copy exists in the objects folder, and dobjd
  knows nothing about it.
- **Written by hand, not by Claude:** the guide quotes recipes, yields,
  durability and level-up amounts from
  `../../../examples/bitcraft/plugin.rhai`, but only the maintainer edits
  it. When a change makes it inaccurate, report the stale passages instead
  of editing it (see `CLAUDE.md`).
- **Generic behavior:** no guide row; the list shows only objects.

## Action icons

Each action gets a 32x32 app-style graphic, chosen by its category (tool
interaction, material creation, raw resource gathering or rekey), drawn at
16x16 in the context menu, at 32x32 in the Recipe Info list, and at 4x in
the Recipe Info Details pane and in the Details pane for a selected job. The
rules, and how to add to them, are in `ACTION_ICONS.md`; the data is
`action-icons.json`. Generic behavior: no action graphics.
