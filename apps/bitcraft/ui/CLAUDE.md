# Bitcraft UI

A single-page web client (`index.html`, no build step) for the dobjd HTTP API, specialised to the `bitcraft` plugin.

For the desktop app, run `just bitcraft-dev` from the repository root. It prepares the bundled runtime and launches Tauri, whose managed daemon serves the UI at `http://127.0.0.1:7727/bitcraft/`. The `bundled-bitcraft-ui` daemon feature injects its own API origin into the page, so UI requests use the same origin rather than a fixed port. The managed MCP endpoint uses port `7728` when enabled.

For standalone browser development, serve this directory with `python3 -m http.server 4170 --bind 127.0.0.1 --directory apps/bitcraft/ui` and open `http://127.0.0.1:4170`. Without the daemon-injected API origin, the page connects to the standalone Digital Objects daemon at `http://127.0.0.1:7717` (MCP uses `7718` when enabled). See [the desktop README](../README.md) for build and bundle commands.

The main window always shows the inventory (the files view). Its top bar
has three groups of icons, separated by thin dividers. The game views: a
folder, permanently selected, for the files view; a bar chart that opens
the Inventory Summary; and an open book that opens Recipe Info. The outside
links: an info icon that opens the Bitcraft website in the player's
browser, and a Discord icon that does nothing yet. Last, a gear that opens
Settings. Recipe Info, the Inventory Summary and
Settings each open their window once, and a later click focuses it. These
windows load this same page in a mode of their own and run none of the
main window's startup.

Settings (`?page=settings`) shows the synchronizer and relayer addresses
from dobjd's `GET /settings` in read-only fields; they cannot be edited yet.

In the desktop app, the host builds every window the page opens. Links to
the daemon open as app windows; links to a site listed in the host's
`EXTERNAL_LINK_HOSTS` (in `src-tauri/src/main.rs`) open in the player's
browser instead; anything else is blocked. A new external link from the
page needs its host added there.

The Inventory Summary (`?page=summary`) is laid out like Finder's Get Info
window: the inventory's Total Bit Value beside an icon, then "General" (the
value and the live object total) and "Live objects" (a count per class)
sections, which can be collapsed. It refreshes on every job event from
dobjd's `/events` stream, when that stream reconnects, and on window focus.
It polls every 10 seconds only while some object is pending or unknown,
since those settle when the inventory is read, without an event.

Recipe Info is loaded as
`?page=recipebook`: it shows only the action list and details, and runs none
of the main window's startup (no inventory, jobs, status bar or starter
pocketknife). Its address tracks the selected action, so
`?page=recipebook&action=bitcraft::GatherWood` (the parameter keeps its
older name) opens Recipe Info with that action selected and shown.

Startup keeps the inventory hidden until the player has a live pocketknife.
Automatic setup jobs stay out of Jobs; manual jobs stay visible. Failures
offer retry and never open the inventory. See `BITCRAFT_SPECIFIC_RULES.md`.
The startup screen is laid out like the details pane (document icon, 16px
heading, muted text, progress bar in the blue accent), and the same layout
appears in the desktop `shell/`, whose `style.css` copies this page's values
as literals. Keep both splash screens visually consistent, including the bottom status
bar, which both show. On this page the bar is live: it reads the service
addresses from dobjd's settings and probes each service. The desktop shell's
"Preparing cryptography..." screen runs before dobjd is ready, so it cannot
do either; its bar is hardcoded static markup that shows "Loading..." for
the synchronizer and relayer statuses and addresses, with a yellow dot by
each status, and a hardcoded version that must match `BITCRAFT_VERSION`
here. The progress bar counts completed milestones: cryptography ready, pocketknife proof generated, and network
confirmation. The step counter and heading describe the current work rather
than the last completed milestone. Errors preserve the bar's completed progress.
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
  pack is generated by `examples/bitcraft/presentation/generate.py`; when
  classes or actions change, update it and run `just bitcraft-presentation`.
  `presentation/` in this folder is a generated, Git-ignored copy: never
  edit it, and run `python3 apps/bitcraft/scripts/sync_presentation.py`
  (or `just bitcraft-presentation`) before serving this folder directly.
- `BITCRAFT_WELCOME.md` is the player's getting-started guide, written and
  maintained by hand by Brian. Never edit, rewrite, rename or regenerate
  it, even when recipes change. It quotes the game's recipes, so when a
  change makes it inaccurate, tell the user which passages are now wrong
  instead. (Brian's local Claude Code settings also deny edits to it.)
- Total Bit Values are hardcoded per class from the recipes (see
  `BITCRAFT_SPECIFIC_RULES.md`). When a recipe's inputs or outputs change,
  recompute them in `index.html` and that file.
