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

### Only bitcraft classes and actions are listed in the Recipe Info window

- **Where:** `PLUGIN_NAME`, applied to the results of `GET /classes` and
  `GET /actions` in `loadReferenceWindow()`, the same filter `load()`
  applies to the actions the context menu offers from.
- **Generic behavior:** list every installed plugin's classes and actions.
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
  pocketknife is listed. Existing live knives, including held knives, open
  the inventory directly once the step on display has been up for its
  minimum (below). Setup runs in four steps, each shown by its heading
  alone:
  1. "Preparing artifacts..." on the desktop shell's screen, while dobjd
     prepares its cryptography (the host can replace the heading with its
     own message);
  2. "Loading inventory..." on this page, while it reads the inventory and
     actions;
  3. "Loading cryptography engine..." while the starter knife's proof is
     made;
  4. "Connecting to Bitcraft network..." while the knife waits for network
     confirmation.
  Both screens share one layout, so the shell leads into this page: the
  Bitcraft logo (the wood axe on a blue folder, the app icon), a 16px
  heading and the progress bar in the app's blue accent. There is no
  description or step number while setup works. The bar counts completed
  steps out of four: the shell shows none done, with a small sliver (0.2 of
  a step) filled so the bar reads as one, successful daemon requests
  complete the second, a committing run, a completed proof event, or an
  existing knife that has not been discarded completes the third, and a
  live knife the fourth. Progress stays at the last completed step during
  errors; retry starts a fresh check from step 2.
  An error replaces the heading with "Bitcraft could not finish setup" and
  adds a description, "Stopped at step N of 4" under the bar, the error
  itself, and "Try again" and "Copy error".
  Each step stays up for at least one second, even when its work finishes
  sooner; the work itself is not delayed. The screen then moves one step at
  a time, skipping steps the startup never reached, and opens the inventory
  only after the last step shown has had its second. Errors and retries are
  shown at once.
  Steps do not represent time remaining. Keep both screens' appearance in
  step when changing either. Both show the bottom status bar: live on this
  page, and hardcoded on the desktop shell's screen, which runs before
  dobjd is ready and shows "Loading..." with a yellow dot for each service
  (see `CLAUDE.md`).
  The status bar stays visible below the setup screen, so service
  status and addresses can be checked while setup runs or after it fails.
  Object changes made while the setup screen is up do not flash in the
  object list, so the starter knife is already settled when inventory
  opens.
- **Jobs:** automatic startup runs are tracked but omitted from Jobs.
  Remember their run ID in session storage so reloading the page keeps
  them hidden. Manual pocketknife jobs, including restored jobs without
  that marker, remain visible. The Jobs pane is hidden when no visible
  jobs exist; starting a manual job reveals it.
- **Failures:** show the cause with "Try again" and "Copy error" controls
  if setup fails, active runs cannot be read, the synchronizer cannot be
  reached before discarding a stale knife, or the recipe is unavailable.
  There is no way into the inventory from a setup error: it opens only
  once a live knife exists. Retry re-reads active jobs and inventory before
  discarding or creating anything.
  When a setup request or progress read gets no HTTP response at all, the
  daemon is not running, so the error says the background service isn't
  responding, suggests quitting and reopening Bitcraft, and names
  `dobjd.log` in the data directory (the parent of the objects directory)
  once that directory is known. An HTTP error is shown as it is.
  "Copy error" copies the full displayed error, including line breaks, and
  reports success or failure. The error text can also be selected manually.
  A failed run is never retried automatically while the page is open;
  reopening the app retries after discarding its stale knife. The startup
  job is followed through its event stream like any other job; the
  terminal status arrives through the read prompted by dobjd closing the
  stream. Inventory is polled after success until the knife is live.
  Inventory refreshes alone do not trigger creation.
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
| `LevelUpQuarry1` .. `LevelUpQuarry5` | the selected `Quarry`'s `level` equals the number N in the action name, the wood bucket's `n` is exactly 20 x 2^N and the stone bucket's exactly 10 x 2^N (`LEVEL_UP_BUCKETS`) | `action.st_equal(quarry.level, N)`, `action.st_equal(woodbucket.n, ...)` and the stone bucket |
| `LevelUpMiningDrill1` .. `LevelUpMiningDrill5` | the selected `MiningDrill`'s `level` equals N, and the wood, stone and iron buckets hold exactly 10, 50 and 5 x 2^N | `action.st_equal(miningdrill.level, N)` and one `st_equal` per bucket |

- **Generic behavior:** offer every action whose input classes match the
  selection. A level-1 drill with 5 wood would show all six
  `UseMiningDrillN` actions.

### Known constraints not yet encoded

- `UseWoodAxe`, `UseStoneAxe` and `UseIronAxe` require the axe's
  `durability` to be greater than 0.

## Cosmetic

- **Page title:** "Bitcraft", which becomes "Bitcraft (N.NNN BIT)" once the
  inventory loads (see below).
- **Presentation pack:** class and action icons, summaries and
  descriptions come from the bitcraft presentation pack
  (`../../../examples/bitcraft/presentation/`), which the app ships by
  listing it in `PACKS` in `../scripts/sync_presentation.py`. The UI reads
  packs through `presentation.js` and has no icon rules of its own; the
  pack's content, and how it is generated, are described in that folder's
  `README.md`. Where they show:
  - class icons on a 16x16 rounded white tile in the object list and beside
    every class name, and at 6x (96x96 CSS px) on the large page icon in a
    single object's Details pane, centered across it and just below its
    folded corner;
  - class icons also at 2x (32x32) in the Recipe Info list, and on the
    large page icon in Recipe Info's Details pane for a class;
  - action icons at 16x16 in the context menu, jobs list and a class's
    action lists in Recipe Info, 32x32 in the Recipe Info list, and
    128x128 in Recipe Info's and a job's Details pane;
  - class and action summaries as the second line of each entry in the
    Recipe Info list (falling back to the manifest's description, then to
    "object class" or "executable action");
  - a "Description" section (summary and description) in Recipe Info's
    Details pane and, for the object's class, in a single object's Details
    pane.
- **Generic behavior:** no pack, so plain page icons, no action icons, and
  only the manifest's descriptions.

## Inventory value, Inventory Summary and status bar

The inventory's value, shown at the right end of the main window's top bar
("Total Inventory Value:", the `SUMMARY_ICON_CLASS` icon and "31.000 BIT", which
opens the Inventory Summary) and in its title ("Bitcraft (31.000 BIT)",
which the desktop app shows in its title bar), the Inventory Summary window
opened
from the top bar, and the status bar along the bottom of the page. Three of
their entries know about bitcraft:

- **Bitcraft version** (status bar): `BITCRAFT_VERSION` ("Alpha 1.0"), a
  fixed string in the page; dobjd and the plugin manifest report no game
  version.
- **Total Inventory Value** (top bar, window title and Inventory Summary):
  the summed
  Total Bit Value of the live bitcraft objects (pending ones are left out);
  see below. The Inventory Summary also shows each class's summed value
  beside its count, and hovering an object row in the main window shows
  that object's value in a tooltip.
- **Inventory Summary icon:** `SUMMARY_ICON_CLASS` (`Bit`), the class whose
  art heads the window, since it totals the inventory in bits.
- **Website and Discord links:** `BITCRAFT_SITE_URL` and
  `BITCRAFT_DISCORD_URL`, opened in the player's browser by the top bar's
  info and Discord icons. Generic behavior: no links.
- **Generic behavior:** no version, Total Inventory Value, per-class values
  or summary icon. The
  Inventory Summary's object count and per-class counts are generic: both
  cover this plugin's live objects. The other entries
  are generic: the synchronizer and relayer statuses and addresses in the
  status bar (the page probes each service's `/healthz` at the URLs
  `GET /settings` reports, every 10 seconds).

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
15 + 135 x 2^K.

Wear lowers the value: an axe is worth its value above times the fraction
of its durability it has left, out of the 100 a new wood axe starts with
(`FULL_DURABILITY`; upgrading to a stone or iron axe carries the durability
over). An axe at 60 durability is worth 60% of a new one, and a spent axe
nothing. Rekeying does not change the value: a `Rekey<Class>` action leaves
every value field alone. Values are shown with three decimal places and the
unit, as in "31.000 BIT".

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

