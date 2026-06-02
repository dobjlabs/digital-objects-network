# bitcraft commands

Source-of-truth SKILL.md files for the **meta commands** that ship with bitcraft. `just install-commands` copies each `<name>/SKILL.md` here to `~/.claude/skills/bitcraft-<name>/SKILL.md`. `just dev` runs this on first boot via `ensure-commands`. `just reset` wipes them.

Bitcraft ships **no per-plugin gameplay commands** — those are authored by the user via `create-command` once a plugin is loaded. The shipped set is the irreducible meta layer:

- `start`     — open the live dashboard pane, print help (hidden from the help list since it's the entry point itself)
- `help`      — render the column-aligned help block
- `preview`   — open / refresh the dashboard pane (hidden from the help list, called by `start`)
- `consult-docs` — query the bitcraft doc resources for a verbatim answer
- `create-command` — define a new bitcraft command

## The three layers

```
                       ┌─ MCP tools (root) ─────────────┐
                       │   1:1 with the driver:         │
                       │   list_inventory, run_action,  │
                       │   inspect_*, check_feasibility,│
                       │   get_state_root, …            │
                       │   not user-facing; only        │
                       │   invoked from inside a command│
                       └────────▲───────────────────────┘
                                │
   ┌─ Meta commands (this dir) ──┐     ┌─ Gameplay commands ───────┐
   │  start, help, preview,      │ ◄─► │  user-authored via         │
   │  consult-docs,              │     │  `create-command`. Written │
   │  create-command             │     │  to ~/.claude/skills/      │
   │                             │     │  bitcraft-<name>/. They    │
   │  installed by               │     │  survive `just install-    │
   │  `just install-commands`    │     │  commands` (not wiped).    │
   └─────────────────────────────┘     └────────────────────────────┘
                                │
                       ┌────────▼───────────────────────┐
                       │  User-facing surface           │
                       │  MCP instructions present only │
                       │  the command list. Three cases:│
                       │   1. listed command (exact name│
                       │      or unambiguous phrase) →  │
                       │      run that skill            │
                       │   2. help → render help block  │
                       │   3. anything else → fallback  │
                       │      line                      │
                       └────────────────────────────────┘
```

The help block is built dynamically: `format_help.py` scans `~/.claude/skills/bitcraft-*/SKILL.md`, parses the frontmatter `name` + `description`, skips entries with `hidden: true`, and renders a column-aligned block. User-authored commands appear automatically once written; `start` and `preview` stay hidden.

## What a command file can contain

The body of a `SKILL.md` may include any combination of:

- **Prose** instructions for Claude.
- **MCP tool calls** — any bitcraft tool the daemon exposes (`run_action`, `list_inventory`, `inspect_*`, etc.).
- **References to other bitcraft commands** by name — Claude triggers them.
- **Scripts in any language** inline in fenced code blocks (`bash`, `python`, `node`, …). Claude executes them via the Bash tool.
- **Sibling files** for longer scripts. `create-command` writes them next to `SKILL.md`. Reference them by absolute path: `~/.claude/skills/bitcraft-<name>/<filename>`.
- **`.pexe` actions** — invoked via `run_action` like any other action. Plugin authoring lives in [../plugins/](../plugins/).
- **Markdown for the user** in user-authored commands. (Meta commands keep strict plain-text output for MUD determinism.)

Not supported yet: a2a bots, direct GUI manipulation.

## Adding a meta command

Rare — these are the framework-level commands. To add one:

1. `mkdir commands/<name>` and write `commands/<name>/SKILL.md`.
2. Frontmatter format:
   ```
   ---
   name: bitcraft-<name>
   description: <one-line, used in the help block>
   hidden: true     # optional — hides it from the help block
   ---
   ```
3. Body should specify exact output for deterministic MUD-style results (see existing meta commands for the pattern).
4. `just install-commands`, then restart Claude Code so it reloads skills.

## Adding a gameplay command (the common case)

Type `create-command` (or `define a new command`) in Claude Code. The meta-command walks you through name, description, body, and any sibling scripts, then writes the result to `~/.claude/skills/bitcraft-<name>/`. Reload Claude Code to register. Gameplay commands typically call `run_action` against the loaded plugin's actions.

## Convention: strict output for meta commands

Each meta command has an `## Output rules` section near the top:

- plain text only, no markdown
- exact result lines (e.g. `<class> → <path>`)
- exact error lines (e.g. `no <class> available — run <hint>`)

This keeps the MUD feel consistent across the meta set. User-authored gameplay commands are not required to follow this — `create-command` says so explicitly.

## Key files

- [../mcp/docs/instructions.md](../mcp/docs/instructions.md) — the MCP system prompt with the dispatch logic.
- [../mcp/src/server.rs](../mcp/src/server.rs) — `enumerate_commands_in`, `parse_skill_meta`, `render_help_block`, `build_instructions_with`.
- [../justfile](../justfile) — `install-commands`, `ensure-commands`, `reset` (wipes `~/.claude/skills/bitcraft-*`, including user-authored).
