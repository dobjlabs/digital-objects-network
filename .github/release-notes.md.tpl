## Install via agent prompt

Paste this to Claude Code, Cursor, or any MCP-aware agent:

> Read https://raw.githubusercontent.com/dobjlabs/zk-craft/main/SKILL.md and set up bitcraft.

The skill installs `dobjd`, `dobj`, `bitcraft-mcp-proxy`, the
`craft-basics` plugin into `~/.dobj/`, and the bitcraft command
skills into `~/.claude/skills/bitcraft-*/`. Then it registers MCP
with the agent so you can drive bitcraft directly.

## Manual install

See [SKILL.md](https://github.com/dobjlabs/zk-craft/blob/main/SKILL.md) for the underlying steps.

## What's in the release

- **`dobjd-{target}.tar.gz`** — the daemon (HTTP API on `:7717`,
  MCP on `:7718`). Bundles `bitcraft-mcp-proxy` + `libscip` + GCC
  runtime libs in `.libs/`.
- **`dobj-{target}.tar.gz`** — terminal CLI for the daemon.
- **`craft-basics.pexe`** — the bundled crafting plugin (Log,
  Wood, Stone, sticks, picks…).
- **`bitcraft-commands.tar.gz`** — the user-facing command skills
  (chop-log, craft-wood, mine-stone, help, start, preview, …)
  installed into `~/.claude/skills/bitcraft-*/`.

Targets: `aarch64-apple-darwin`, `x86_64-apple-darwin`,
`x86_64-unknown-linux-gnu`.

## Notes

- macOS binaries are codesigned + notarized
- Defaults: synchronizer `${DEFAULT_SYNCHRONIZER_API_URL}`, relayer
  `${DEFAULT_RELAYER_API_URL}`. Change with
  `dobj settings set --synchronizer ... --relayer ...`.
