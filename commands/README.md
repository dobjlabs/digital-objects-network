# bitcraft commands

Source-of-truth SKILL.md files for bitcraft commands. `just install-commands` copies each `<name>/SKILL.md` here to `~/.claude/skills/bitcraft-<name>/SKILL.md`. `just dev` runs this on first boot via `ensure-commands`. `just reset` wipes them.

## The four layers

```
                          ┌─ MCP tools (root) ────────────────┐
                          │   1:1 with the driver:            │
                          │   list_inventory, run_action,     │
                          │   inspect_*, check_feasibility,   │
                          │   get_state_root, …               │
                          │   not user-facing; only invoked   │
                          │   from inside a command           │
                          └────────▲──────────────────────────┘
                                   │
       ┌─ Commands (this directory) ─┐
       │  built-in:                  │
       │  chop-log, craft-wood,      │      ┌─ Custom commands ─┐
       │  craft-sticks,              │ ◄──► │  user-authored    │
       │  craft-wood-pick,           │      │  via create-command│
       │  mine-stone,                │      │  written to       │
       │  craft-stone-pick,          │      │  ~/.claude/skills/│
       │  create-command (meta)      │      │    bitcraft-*/    │
       └─────────────────────────────┘      └───────────────────┘
                                   │
                          ┌────────▼──────────────────────────┐
                          │  User-facing surface              │
                          │  MCP instructions present only    │
                          │  the command list. Three cases:   │
                          │   1. help → render help block     │
                          │   2. listed command (exact name   │
                          │      or unambiguous phrase) →     │
                          │      run that skill               │
                          │   3. anything else → fallback     │
                          │      line                         │
                          └───────────────────────────────────┘
```

Help is built dynamically at every MCP `initialize`: dobjd scans `~/.claude/skills/bitcraft-*/SKILL.md`, parses the frontmatter `name` + `description`, and renders a column-aligned block. Custom commands appear automatically once installed.

## What a command file can contain

The body of a `SKILL.md` may include any combination of:

- **Prose** instructions for Claude.
- **MCP tool calls** — any bitcraft tool the daemon exposes (`run_action`, `list_inventory`, `inspect_*`, etc.).
- **References to other bitcraft commands** by name — Claude triggers them.
- **Scripts in any language** inline in fenced code blocks (`bash`, `python`, `node`, …). Claude executes them via the Bash tool.
- **Sibling files** for longer scripts. `create-command` writes them next to `SKILL.md`. Reference them by absolute path: `~/.claude/skills/bitcraft-<name>/<filename>`.
- **`.pexe` actions** — invoked via `run_action` like any other action. Plugin authoring lives in [../plugins/](../plugins/).
- **Markdown for the user** in user-authored commands. (Built-ins keep strict plain-text output for MUD determinism.)

Not supported yet: a2a bots, direct GUI manipulation.

## Adding a built-in command

1. `mkdir commands/<name>` and write `commands/<name>/SKILL.md`.
2. Frontmatter format:
   ```
   ---
   name: bitcraft-<name>
   description: <one-line, used in the help block>
   ---
   ```
3. Body should specify exact output for deterministic MUD-style results (see existing built-ins for the pattern).
4. `just install-commands`, then restart Claude Code so it reloads skills.

## Adding a custom command

Type `create-command` (or `define a new command`) in Claude Code. The meta-command walks you through name, description, body, and any sibling scripts, then writes the result to `~/.claude/skills/bitcraft-<name>/`. Reload Claude Code to register.

## Convention: strict output for built-ins

Each built-in has an `## Output rules` section near the top:

- plain text only, no markdown
- exact result lines (e.g. `Log → <path>`)
- exact error lines (e.g. `no Log available — run chop-log`)

This keeps the MUD feel consistent across the built-in set. Custom commands are not required to follow this — `create-command` says so explicitly.

## Key files

- [../mcp/docs/instructions.md](../mcp/docs/instructions.md) — the MCP system prompt with the three-case dispatch. `{{COMMANDS}}` is substituted at runtime with the help block.
- [../mcp/src/server.rs](../mcp/src/server.rs) — `enumerate_commands_in`, `parse_skill_meta`, `render_help_block`, `build_instructions_with`.
- [../justfile](../justfile) — `install-commands`, `ensure-commands`, `reset` (which now wipes `~/.claude/skills/bitcraft-*`).
