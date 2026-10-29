# Bitcraft UI

A single-page web client (`index.html`, no build step) for the dobjd HTTP API, specialised to the `bitcraft` plugin.

For the desktop app, run `just bitcraft-dev` from the repository root. It prepares the bundled runtime and launches Tauri, whose managed daemon serves the UI at `http://127.0.0.1:7727/bitcraft/`. The `bundled-bitcraft-ui` daemon feature injects its own API origin into the page, so UI requests use the same origin rather than a fixed port. The managed MCP endpoint uses port `7728` when enabled.

For standalone browser development, serve this directory with `python3 -m http.server 4170 --bind 127.0.0.1 --directory apps/bitcraft/ui` and open `http://127.0.0.1:4170`. Without the daemon-injected API origin, the page connects to the standalone Digital Objects daemon at `http://127.0.0.1:7717` (MCP uses `7718` when enabled). See [the desktop README](../README.md) for build and bundle commands.

The main window always shows the inventory (the files view). Its top bar has three groups of controls, separated by thin dividers, and at its right end the inventory's value, read like a balance ("Total Inventory Value:", the Bit icon and "N.NN BIT"), which opens the Inventory Summary. Just left of it, past a divider, "N.NN BIT/sec" shows how fast that value grew over the last 5 minutes (`VALUE_RATE_WINDOW_MS`): the value now minus the value at the window's start, divided by the full 300 seconds, from samples taken on every inventory change and kept in session storage so a reload does not restart it. Before the first sample the value counts as that sample's, so a new session's first gains do not spike. It refreshes every second and can be negative. The window title repeats both: "Bitcraft (N.NN BIT - N.NN BIT/sec)". A rate that rounds to 0.00 is left out of both, with its divider, and the title is then "Bitcraft (N.NN BIT)". In the desktop the rate comes from the native host instead, which keeps sampling while the window is hidden; the page reads it through `inventory_metrics` (see [the desktop README](../README.md#discord-rich-presence)). The game views: a folder, permanently selected, for the files view; a bar chart that opens the Inventory Summary; and an open book that opens Recipe Info. Then the desktop agent buttons, drawn slightly smaller than the other icons since their logo marks fill more of the box, with the tooltips "Automate with Claude" and "Automate with Codex". They use monochrome Claude and ChatGPT/OpenAI logo marks. They open Claude Code or Codex with the game's home directory and a player prompt; these controls are hidden in a standalone browser. The host handles their `bitcraft://agent/claude` and `bitcraft://agent/codex` requests, constructing the external deep links from its own installation and game home paths. The app and tray menus also offer "Play with agents...", a native guide window backed by `shell/agent.html`. It shares the startup shell's base styling and shows the same player prompt as the deep links, with copy controls; installation paths are included in the prompt. Missing URL handlers and launch errors open this guide with an explanatory message on macOS, Windows and Linux. Last, a gear opens Settings. Toolbar tooltips are drawn by the page's CSS (`data-tooltip`), so they appear without the system's hover delay. Recipe Info, the Inventory Summary and Settings each open their window once, and a later click focuses it. These windows load this same page in a mode of their own and run none of the main window's startup.

"View raw", Open and a double-click on a row open `?page=raw&file=NAME`, a
window that shows the file's stored text (an object's from dobjd's
`GET /objects/{file_name}/raw`, or the welcome guide's Markdown), titled with
the file's name. It opens wide enough for the title bar to show the whole
name (the name measured in the title font, plus room for the window buttons;
at least 480px). It sets its own light background, since the desktop app's
webview shows a bare JSON response in the system's dark theme, and makes its
text selectable, since the desktop app turns selection off on every page
and the windows it opens inherit that.

The outer page fills the window with scrolling and overscroll disabled, including on the desktop startup screen. Scrollable content stays within its inventory, jobs, details or secondary-view panel.

Settings (`?page=settings`) shows the synchronizer and relayer addresses
from dobjd's `GET /settings` in read-only fields; they cannot be edited yet.
Below them, set apart like a Mac About window, it shows the Bitcraft name
and version, the website's address as a link, and a Discord button; both
open in the player's browser. The window's opening height counts the title
bar in the desktop app, so it leaves room for one.

In the desktop app, the host builds every window the page opens. Links to
the daemon open as app windows; links to a site listed in the host's
`EXTERNAL_LINK_HOSTS` (in `src-tauri/src/main.rs`) open in the player's
browser instead; anything else is blocked. The windows it opens, such as
Settings, send listed sites to the browser the same way and open nothing
else. A new external link from the page needs its host added there.

The Inventory Summary (`?page=summary`) is laid out like Finder's Get Info window: the inventory's Total Inventory Value beside an icon, then an "Objects (N)" section, N being the live bitcraft objects it lists, with a count and a summed value per class, which can be collapsed. Both inventory views apply `object` and `object-removed` events from dobjd's `/events` stream, including changes made by other clients. They read inventory and transaction snapshots on load, stream connection or reconnection, sequence gaps, and window focus. Changes received during a snapshot read are reapplied to its result. Recorded transactions settle in the daemon and need no UI polling; pending or unknown objects with no open local transaction record are reconciled every 10 seconds.

Actions using independent inputs can run concurrently, including actions with no inputs. Selected inputs must be live and unheld. The page reserves inputs while the start request is pending and until the action finishes; daemon holds also cover work started in another window or client.

Jobs discovers work from every client through `/events`. A run, proof job and transaction describing the same action share one row, linked by run/proof-job IDs, transaction ownership and `txFinal`. Recipe names and shared owner labels do not group independent actions. Run-backed jobs retain their replayable run stream; separately proved actions show "Ready to submit" until submission and remain active through settlement. Details include proof/transaction IDs, input and output paths, status, errors and available progress. Retained runs and proof jobs are read on load, stream open/reopen, sequence gaps and window focus. Open transaction records recover activity after a daemon restart. Historical transactions without retained activity are left out of Jobs. An unfinished proof lost on restart shows an error unless its transaction survived; that transaction continues through settlement. Inventory is not re-read for each progress event. Completed transaction receipts are pruned after ten minutes; `transaction-pruned` removes them from the inventory's transaction snapshot without changing completed Jobs to failures.

Recipe Info is loaded as
`?page=recipebook`: it shows only a list of classes, then actions, with the
selected one's details, and runs none of the main window's startup (no
inventory, jobs, status bar or starter pocketknife). A class's details list
the actions it is an output of and an input to, which select that action
when clicked, and a collapsed "Class Source" holding the class's `Is<Class>`
predicate from `GET /classes` followed by the predicate of every action
that outputs it, each after a `//` line naming it. Its address tracks the
selection, so `?page=recipebook&class=bitcraft::Wood` or
`?page=recipebook&action=bitcraft::GatherWood` (the page parameter keeps its
older name) opens Recipe Info with that entry selected and shown.

Resting the pointer on a row of the object list for a quarter second
(`ROW_TOOLTIP_DELAY_MS`) shows a tooltip with the object's Total Bit Value
alone; the welcome guide's row shows none. The welcome guide's name is set
in semibold, so it stands out from the objects below it. Capital goods (see `BITCRAFT_SPECIFIC_RULES.md`) come next, in bold, ahead of the other objects, with their level on a badge over their icon's bottom right corner, colored by level from green (1) to gold (6 and above), here and in the Details pane. Buckets carry the quantity they hold on a dark gray badge in the same corner, abbreviated ("5.3K", "53K"), in both places. Names are cut to
fit in each row's own font. Rows carry their file name in `data-file-name` rather than a `title`, so
no delayed native tooltip competes with it; the tooltip is a single element
outside the list, placed by the script, since the scrolling list would clip
it.

Recipe Info's Details pane shows, under the hash, the `.pexe` a class or
action is "defined in". dobjd reports that path only with actions, so a
class shows the path of its plugin's actions.

Class and action names in a Details pane are links to Recipe Info: the
class of each selected object and a job's action in the main window, and an
action's input and output classes and a class's actions in Recipe Info
itself. Inside Recipe Info a link selects the entry. Elsewhere it opens
Recipe Info at the entry; if that window is already open, the page asks it
to select the entry, through a function it exposes for this, and sends it
to the entry's address only when it cannot (as while its list loads).

Startup keeps the inventory hidden until the player has a live pocketknife.
Automatic setup jobs stay out of Jobs; manual jobs stay visible. Failures
offer retry and never open the inventory. See `BITCRAFT_SPECIFIC_RULES.md`.
The startup screen shows the Bitcraft logo, a 16px heading and a progress
bar in the blue accent, and the same layout appears in the desktop `shell/`,
whose `style.css` copies this page's values as literals. The logo is the app
icon (`../src-tauri/icons/icon.png`), copied to `assets/bitcraft-logo.png`
here and `../shell/bitcraft-logo.png`; replace all three together. Keep both
splash screens visually consistent, including the bottom status bar, which
both show. On this page the bar is live: it reads the service addresses from
dobjd's settings and probes each service. The desktop shell's "Preparing
artifacts..." screen runs before dobjd is ready, so it cannot do either; its
bar is hardcoded static markup that shows "Loading..." for the synchronizer
and relayer statuses and addresses, with a yellow dot by each status, and a
hardcoded version that must match `BITCRAFT_VERSION` here. The progress bar
counts four steps: the shell's preparation, loading the inventory, the
starter knife's proof, and its network confirmation. Only an error shows a
description and the step setup stopped at. Errors preserve the bar's
completed progress.
Each step stays up at least one second (`STARTUP_STEP_MIN_MS`) even when its
work finishes sooner. The tests' page harness ends these holds at the next
turn of the event loop; pass `holdSteps` to keep them until `elapse()`.

Run browser behavior checks with `pnpm --dir apps/bitcraft/tests install --frozen-lockfile`
and `pnpm --dir apps/bitcraft/tests test` from the repository root. The tests
use a simulated daemon and need no Tauri host or proving artifacts.

## Bitcraft-specific rules

`BITCRAFT_SPECIFIC_RULES.md` lists every place where this UI behaves
differently than it would for a generic Digital Objects store, because it
knows something about the `bitcraft` plugin that the API does not report
(scope filters, action input constraints, naming).

- Read it before changing the UI.
- Whenever a change adds, alters, or removes bitcraft-specific behavior,
  update that file in the same change.
- Keep bitcraft-specific logic inside the `Bitcraft-specific rules` block at
  the top of the script in `index.html`; code outside that block should stay
  plugin-agnostic.
- If a change to `examples/bitcraft/plugin.rhai` alters an action's input
  constraints, update both `ACTION_INPUT_RULES` and the file.
- Icons, summaries and descriptions come from presentation packs (see
  `../PRESENTATION_PACKS.md`), read through `presentation.js`. Bitcraft's
  pack is generated by `examples/bitcraft/presentation/generate.py` from
  the art and the hand-written copy in that folder's `copy/`; when classes
  or actions change, update them and run `just bitcraft-presentation`.
  `presentation/` in this folder is a generated, Git-ignored copy: never
  edit it, and run `python3 apps/bitcraft/scripts/sync_presentation.py`
  (or `just bitcraft-presentation`) before serving this folder directly.
- `BITCRAFT_WELCOME.md` is the player's getting-started guide, written and
  maintained by hand by Brian. Never edit, rewrite, rename or regenerate
  it, even when recipes change. It quotes the game's recipes, so when a
  change makes it inaccurate, tell the user which passages are now wrong
  instead. (Brian's local Claude Code settings also deny edits to it.)
- The summaries and descriptions in
  `examples/bitcraft/presentation/copy/` are also written by hand by Brian.
  Never edit them; tell the user which entries a change makes wrong or
  missing instead (see that folder's `CLAUDE.md`).
- Total Bit Values are hardcoded per class from the recipes (see
  `BITCRAFT_SPECIFIC_RULES.md`). When a recipe's inputs or outputs change,
  recompute them in `inventory-values.json` and that file.
