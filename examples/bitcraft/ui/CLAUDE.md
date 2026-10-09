# Bitcraft UI

A single-page web client (`index.html`, no build step) for the dobjd HTTP
API, specialised to the `bitcraft` plugin. It talks to dobjd at
`http://127.0.0.1:7717`; serve it with any static file server, e.g.
`python3 -m http.server 4170 --bind 127.0.0.1 --directory examples/bitcraft/ui`.

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
