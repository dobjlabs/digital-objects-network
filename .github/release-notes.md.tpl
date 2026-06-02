## Install via agent prompt

Paste this to Claude Code, Cursor, or any MCP-aware agent:

> Read https://raw.githubusercontent.com/dobjlabs/zk-craft/main/SKILL.md and set up bitcraft.

The skill installs `dobjd`, `dobj`, `bitcraft-mcp-proxy`, the
`${EPISODE}` plugin into `~/.dobj/`, and the bitcraft command
skills into `~/.claude/skills/bitcraft-*/`. Then it registers MCP
with the agent so you can drive bitcraft directly.

## Manual install

See [SKILL.md](https://github.com/dobjlabs/zk-craft/blob/main/SKILL.md) for the underlying steps.

## What's in the release

- **`dobjd-{target}.tar.gz`** — the daemon (HTTP API on `:7717`,
  MCP on `:7718`). Bundles `bitcraft-mcp-proxy` + `libscip` + GCC
  runtime libs in `.libs/`.
- **`dobj-{target}.tar.gz`** — terminal CLI for the daemon.
- **`${EPISODE}.pexe`** — the bundled crafting plugin
  (classes, actions, and recipe predicates this release ships with).
  Inspect with `dobj actions` / `dobj classes` after installing.
- **`bitcraft-commands.tar.gz`** — the framework-level command
  skills (`help`, `consult-docs`, `create-command`, plus internal
  `start` / `preview`) installed into `~/.claude/skills/bitcraft-*/`.
  Gameplay commands are authored by the user via `create-command`
  — none ship in the release.

Targets: `aarch64-apple-darwin`, `x86_64-apple-darwin`,
`x86_64-unknown-linux-gnu`.

## Notes

- macOS binaries are codesigned + notarized
- Defaults: synchronizer `${DEFAULT_SYNCHRONIZER_API_URL}`, relayer
  `${DEFAULT_RELAYER_API_URL}`. Change with
  `dobj settings set --synchronizer ... --relayer ...`.
