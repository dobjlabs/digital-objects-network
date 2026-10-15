# Bitcraft UI

A single-page web client (`index.html`, no build step) for the dobjd HTTP API, specialised to the `bitcraft` plugin.

For the desktop app, run `just bitcraft-dev` from the repository root. It prepares the bundled runtime and launches Tauri, whose managed daemon serves the UI at `http://127.0.0.1:7727/bitcraft/`. The `bundled-bitcraft-ui` daemon feature injects its own API origin into the page, so UI requests use the same origin rather than a fixed port. The managed MCP endpoint uses port `7728` when enabled.

For standalone browser development, serve this directory with `python3 -m http.server 4170 --bind 127.0.0.1 --directory apps/bitcraft/ui` and open `http://127.0.0.1:4170`. Without the daemon-injected API origin, the page connects to the standalone Digital Objects daemon at `http://127.0.0.1:7717` (MCP uses `7718` when enabled). See [the desktop README](../README.md) for build and bundle commands.

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
- Action icon rules live in `ACTION_ICONS.md` (bitcraft-specific). When
  actions are added or changed, update `action-icons.json` and that file
  together.
