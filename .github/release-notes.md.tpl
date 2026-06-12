## Install via agent prompt

Paste this to Claude Code, Cursor, or any MCP-aware agent:

> Read https://raw.githubusercontent.com/dobjlabs/digital-objects-network/main/INSTALL.md, install and start the Digital Objects driver, install the craft-rocket plugin, and configure MCP for this agent if supported.

The guide installs `dobjd`, `dobj`, and `dobj-mcp-proxy` into `~/.dobj/`.
The prompt also asks the agent to install the `craft-rocket` plugin and
register MCP so you can drive Digital Objects directly.

## Manual install

See [INSTALL.md](https://github.com/dobjlabs/digital-objects-network/blob/main/INSTALL.md) for the underlying steps.

## Upgrading

Already installed? Run `dobj update` to upgrade in place to this release. It
swaps `dobj`, `dobjd`, and `dobj-mcp-proxy` as a unit (atomic, with
rollback) and leaves your plugins under `~/.dobj/actions/` untouched.

## What's in the release

- **`dobjd-{target}.tar.gz`** — the daemon (HTTP API on `:7717`,
  MCP on `:7718`). Bundles `dobj-mcp-proxy` alongside.
- **`dobj-{target}.tar.gz`** — terminal CLI for the daemon.
- **`*.pexe`** — the example plugins, one archive per plugin under
  `examples/` (e.g. `craft-basics.pexe`, `craft-rocket.pexe`).

Plus `synchronizer-{target}.tar.gz`, `relayer-{target}.tar.gz`, and
(Linux/macOS only) `archiver-{target}.tar.gz` — server binaries used by
the install-test CI workflow. End users don't need these; the installer
doesn't fetch them.

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
