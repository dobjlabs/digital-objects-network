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

- **Where:** `PLUGIN_NAME`, applied in `showObjects()` to inventory snapshots and object events.
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

### Recipe Info lists the classes in game order

- **Where:** `REFERENCE_CLASS_ORDER`, applied in `loadReferenceWindow()`
  through `compareReferenceClasses()`.
- **What:** Pocketknife, Wood, Stone, Iron, Bit, Stick, WoodAxe, StoneAxe,
  IronAxe, Quarry, MiningDrill, BucketOfWood, BucketOfStone, BucketOfIron.
  Classes not listed follow, by name.
- **Generic behavior:** the order `GET /classes` reports, which is by name.
- **Why:** the starter tool, the resources and what they make, then the
  tools, machines and buckets, roughly in the order a player meets them.

### Only bitcraft activity appears in Jobs

- **Where:** `activityInScope()`, applied to retained `/runs`, `/proof-jobs`, open transaction records and activity arriving on `/events`.
- **Generic behavior:** show actions for every plugin, including work started by the CLI, MCP or another window. A run, proof job and transaction linked by IDs describe one action and share one row; independent actions stay separate. Retained completed work also appears when recovering missed events.
- **Why:** same scope as the object list.

## Startup pocketknife

- **Where:** `ensureStartupPocketknife()`, called by `load()` after retained activity has been restored successfully.
- **What:** if no live `bitcraft::Pocketknife` exists, start
  `bitcraft::CraftPocketknife` with no inputs. Live knives, including knives
  held by a run, count as existing. An active crafting run or another active
  action that outputs a pocketknife suppresses creation. Re-read inventory
  after restoring runs so a run that finished during startup does not cause
  another knife to be crafted.
- **Durable transactions:** if the run registry was lost, an open transaction that creates a pocketknife suppresses another craft. Startup follows its transaction and object events, keeping its outputs. A recorded transaction with no submission attempt is submitted using `/transactions/submit`; an attempted submission with an unknown outcome is followed until landing or expiry. Failure to read transaction records stops setup. Expiry or deletion shows an error and requires an explicit retry before creating another knife.
- **Stale knives:** with no live knife, active crafting run, or open pocketknife transaction, a pending or unknown knife has no work left that could make it live. The inventory re-read has already checked it against the synchronizer, so it is moved aside with `POST /objects/{file_name}/discard_unsafe`, which keeps none of dobjd's object-state guarantees, and a new knife is crafted. This happens on "Try again" after a failed commit and on reopening the app. Discarding first requires the synchronizer to answer `GET /state-root`, because the inventory falls back to local state when it is unreachable; otherwise setup shows an error and keeps the knife.
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
- **Jobs:** automatic startup runs are tracked but omitted from Jobs. Remember their run ID in session storage so reloading the page keeps them hidden, including when only their owned transaction remains. Manual pocketknife jobs, including externally started proofs and restored transactions without that marker, remain visible. A proof already creating a pocketknife suppresses another automatic craft. The Jobs pane is hidden when no visible jobs exist; starting a manual job reveals it.
- **Failures:** show the cause with "Try again" and "Copy error" controls if setup fails, active runs cannot be read, the synchronizer cannot be reached before discarding a stale knife, or the recipe is unavailable. There is no way into the inventory from a setup error: it opens only once a live knife exists. Retry re-reads active jobs and inventory before discarding or creating anything. When a setup request or progress read gets no HTTP response at all, the daemon is not running, so the error says the background service isn't responding, suggests quitting and reopening Bitcraft, and names `dobjd.log` in the data directory (the parent of the objects directory) once that directory is known. An HTTP error is shown as it is. "Copy error" copies the full displayed error, including line breaks, and reports success or failure. The error text can also be selected manually. A failed run is never retried automatically while the page is open; reopening the app retries after discarding its stale knife. The startup job is followed through its event stream like any other job; the terminal status arrives through the read prompted by dobjd closing the stream. Transaction and object events advance setup, including after the run registry is lost. Inventory changes alone do not trigger creation.
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

- **Page title:** "Bitcraft", which becomes "Bitcraft (N.NN BIT - N.NN
  BIT/sec)" once the inventory loads, or "Bitcraft (N.NN BIT)" while the
  rate is 0.00 (see below).
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
("Total Inventory Value:", the `SUMMARY_ICON_CLASS` icon and "31.00 BIT", which
opens the Inventory Summary) and in its title ("Bitcraft (31.00 BIT - 0.37 BIT/sec)",
which the desktop app shows in its title bar), the Inventory Summary window
opened
from the top bar, and the status bar along the bottom of the page. Three of
their entries know about bitcraft:

- **Bitcraft version** (status bar): `BITCRAFT_VERSION` ("Alpha 1.0"), a
  fixed string in the page; dobjd and the plugin manifest report no game
  version.
- **BIT/sec** (top bar and window title): how much the Total Inventory
  Value grew over the last `VALUE_RATE_WINDOW_MS` (5 minutes), divided by
  the full window, from samples of it taken on every inventory change; the
  value before the first sample counts as that sample's, so a new session's
  first gains do not spike. A rate that rounds to 0.00 is not shown. Since crafting converts value
  rather than creating it, this is in effect the player's production rate.
  Generic behavior: no rate.
- **Total Inventory Value** (top bar, window title and Inventory Summary):
  the summed
  Total Bit Value of the live bitcraft objects (pending ones are left out);
  see below. The Inventory Summary also shows each class's summed value
  beside its count, and hovering an object row in the main window shows
  that object's value in a tooltip. The main game page reports the numeric
  total and five-minute production rate through the desktop's
  `update_inventory_metrics` Tauri command whenever inventory changes and
  every second. Discord Rich Presence shows both values when sharing is
  enabled, including zero and negative rates. These updates use the computed
  numbers, independently of title and toolbar text. Browser and secondary
  pages do not send them; the host accepts only the main game window.
- **Inventory Summary icon:** `SUMMARY_ICON_CLASS` (`Bit`), the class whose
  art heads the window, since it totals the inventory in bits.
- **Website and Discord links:** `BITCRAFT_SITE_URL` and
  `BITCRAFT_DISCORD_URL`, shown as the website's address and a Discord
  button under the name and version (`BITCRAFT_VERSION`) at the foot of the
  Settings window, and opened in the player's browser.
  Generic behavior: no links.
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
every value field alone. Values are shown with two decimal places and the
unit, as in "31.00 BIT".

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

