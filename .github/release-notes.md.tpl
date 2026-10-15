## Bitcraft desktop

Bitcraft installers include the game UI, managed Digital Objects daemon, CLI tools, and Bitcraft plugin:

- **`bitcraft-<version>-aarch64-apple-darwin.dmg`**: macOS Apple Silicon. Open the DMG and drag Bitcraft to Applications.
- **`bitcraft-<version>-x86_64-apple-darwin.dmg`**: macOS Intel. Install in the same way.
- **`bitcraft-<version>-x86_64-pc-windows-msvc.exe`**: Windows per-user installer.
- **`bitcraft-<version>-x86_64-unknown-linux-gnu.AppImage`**: Linux. Make the file executable and run it.

macOS packages are signed, notarized, and stapled. Windows installers are currently unsigned. Bitcraft stores data in `~/.bitcraft` and uses HTTP port `7727` (MCP `7728` when enabled), separately from standalone Digital Objects. Quit Bitcraft before installing a replacement version; existing objects and settings are preserved. Automatic app updates and command-line PATH integration are not enabled yet.

## Standalone Digital Objects: install via agent prompt

Paste this to Claude Code, Cursor, or any MCP-aware agent:

> Read https://raw.githubusercontent.com/dobjlabs/digital-objects-network/main/INSTALL.md, install and start the Digital Objects driver, install the craft-rocket plugin, and configure MCP for this agent if supported.

The guide installs `dobjd`, `dobj`, `dobj-mcp-proxy`, and `pexe` into
`~/.dobj/`. The prompt also asks the agent to install the `craft-rocket`
plugin and register MCP so you can drive Digital Objects directly.

## Manual install

See [INSTALL.md](https://github.com/dobjlabs/digital-objects-network/blob/main/INSTALL.md) for the underlying steps.

## Upgrading

Already installed? Run `dobj update` to upgrade in place to this release. It
swaps `dobj`, `dobjd`, `dobj-mcp-proxy`, and `pexe` as a unit (atomic, with
rollback) and leaves your plugins under `~/.dobj/actions/` untouched.

## What's in the release

- **`dobjd-{target}.tar.gz`** — the daemon (HTTP API on `:7717`,
  MCP on `:7718`). Bundles `dobj-mcp-proxy` alongside.
- **`dobj-{target}.tar.gz`** — terminal CLI for the daemon.
- **`pexe-{target}.tar.gz`** — packaging tool for building and inspecting
  `.pexe` plugin archives. The installer drops it in `~/.dobj/bin` alongside
  `dobj`; plugin authors use it to produce the `*.pexe` files below.
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
