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
  MCP on `:7718`). Bundles `bitcraft-mcp-proxy` alongside.
- **`dobj-{target}.tar.gz`** — terminal CLI for the daemon.
- **`${EPISODE}.pexe`** — the bundled crafting plugin
  (classes, actions, and recipe predicates this release ships with).
  Inspect with `dobj actions` / `dobj classes` after installing.
- **`bitcraft-commands.tar.gz`** — the framework-level command
  skills (`help`, `consult-docs`, `create-command`, plus internal
  `start` / `preview`) installed into `~/.claude/skills/bitcraft-*/`.
  Gameplay commands are authored by the user via `create-command`
  — none ship in the release.

Plus `synchronizer-{target}.tar.gz` and `relayer-{target}.tar.gz` —
server binaries used by the install-test CI workflow. End users don't
need these; the SKILL.md install path doesn't fetch them.

Targets: `aarch64-apple-darwin`, `x86_64-apple-darwin`,
`x86_64-unknown-linux-gnu`, `x86_64-pc-windows-msvc`.

## Notes

- macOS binaries are codesigned + notarized — first run gets an online
  ticket check, no Gatekeeper warning.
- Windows binaries are **not codesigned yet** — first run shows a
  SmartScreen warning ("Windows protected your PC… Run anyway"). We'll
  add an Authenticode cert when one's available.
- Defaults: synchronizer `${DEFAULT_SYNCHRONIZER_API_URL}`, relayer
  `${DEFAULT_RELAYER_API_URL}`. Change with
  `dobj settings set --synchronizer ... --relayer ...`.
