# Installing the Digital Objects driver

Installing the driver gives you:

- `~/.dobj/bin/dobjd` (`dobjd.exe` on Windows) — long-running driver daemon serving:
  - REST/SSE on `http://127.0.0.1:7717`
  - MCP on `http://127.0.0.1:7718/mcp` (off by default; see [Connect an agent](#connect-an-agent-mcp))
- `~/.dobj/bin/dobj` — terminal CLI that talks to dobjd
- `~/.dobj/bin/dobj-mcp-proxy` — stdio↔HTTP bridge for agents that only speak stdio (e.g. Claude Desktop)

The driver installs with an **empty action catalog** — applications ship as
plugins (`.pexe` files). See [Next steps](#next-steps) for installing one,
e.g. **craft-basics**, a small crafting demo, or **craft-rocket**, a larger
factory tech tree.

Installing via an agent? Paste this to Claude Code, Cursor, or any
MCP-aware agent instead of following this page by hand:

> Read https://raw.githubusercontent.com/dobjlabs/digital-objects-network/main/INSTALL.md, install and start the Digital Objects driver, install the craft-rocket plugin, and configure MCP for this agent if supported.

## Install

**macOS / Linux:**

```bash
curl -fsSL https://raw.githubusercontent.com/dobjlabs/digital-objects-network/main/install.sh | sh
```

**Windows (PowerShell):**

```powershell
irm https://raw.githubusercontent.com/dobjlabs/digital-objects-network/main/install.ps1 | iex
```

The installer detects your platform, downloads the latest published release,
and installs into `~/.dobj/bin`. It never prompts; it prints what to do next.
To pin a version, set `DOBJ_VERSION` (bash) / `$env:DOBJ_VERSION` (PowerShell)
to a tag from the [Releases page](https://github.com/dobjlabs/digital-objects-network/releases)
first. To upgrade an existing install, run `dobj update` (or re-run the
installer, which also pulls the latest release).

## Start and verify

```bash
~/.dobj/bin/dobj start         # first start builds ZK circuits — can take a few minutes
~/.dobj/bin/dobj status        # pid + HTTP healthcheck
~/.dobj/bin/dobj state-root    # confirms the hosted synchronizer is reachable (64-hex root)
```

(Windows: `& "$env:USERPROFILE\.dobj\bin\dobj.exe" start` etc.)

`dobj actions` will be empty until you install a plugin — that's expected.

**First-run note (Windows):** the binaries aren't codesigned yet, so Windows
SmartScreen may show "Windows protected your PC" → click **More info → Run
anyway**.

## Next steps

### Install example plugins

`dobj install` takes a local `.pexe` path or an http(s) URL and hot-reloads
the daemon's action catalog — no restart needed. Two example plugins ship with
each release:

- **craft-basics** — a small crafting demo.
- **craft-rocket** — a larger factory tech tree with a rocket win condition.

Install either example, or run both commands to load both.

**macOS / Linux:**

```bash
# Starter demo:
~/.dobj/bin/dobj install https://github.com/dobjlabs/digital-objects-network/releases/latest/download/craft-basics.pexe

# Larger factory demo:
~/.dobj/bin/dobj install https://github.com/dobjlabs/digital-objects-network/releases/latest/download/craft-rocket.pexe

~/.dobj/bin/dobj actions
```

**Windows:**

```powershell
# Starter demo:
& "$env:USERPROFILE\.dobj\bin\dobj.exe" install https://github.com/dobjlabs/digital-objects-network/releases/latest/download/craft-basics.pexe

# Larger factory demo:
& "$env:USERPROFILE\.dobj\bin\dobj.exe" install https://github.com/dobjlabs/digital-objects-network/releases/latest/download/craft-rocket.pexe

& "$env:USERPROFILE\.dobj\bin\dobj.exe" actions
```

### Connect an agent (MCP)

dobjd can serve MCP at `http://127.0.0.1:7718/mcp`, but it's **off by
default**. Turn it on first (takes effect immediately, persists across
restarts):

```bash
dobj settings set --mcp on
```

Installing the binaries does not register MCP with any agent — do that per
agent:

**Claude Code** (idempotent, safe to re-run):

```bash
claude mcp remove dobj 2>/dev/null || true
claude mcp add --transport http dobj http://127.0.0.1:7718/mcp
```

Takes effect on the next Claude Code session.

**Claude Desktop** only speaks stdio, so point it at the bundled
`dobj-mcp-proxy` binary. Merge this into `mcpServers` in its config —
macOS: `~/Library/Application Support/Claude/claude_desktop_config.json`,
Windows: `%APPDATA%\Claude\claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "dobj": {
      "command": "<absolute path to dobj-mcp-proxy[.exe]>",
      "args": ["--port", "7718"]
    }
  }
}
```

Write the file as UTF-8 **without a BOM** (Claude Desktop's JSON parser
silently loads zero MCP servers otherwise), then fully quit and reopen
Claude Desktop.

**Other agents** (Cursor, Aider, Continue, …): point their MCP configuration
at `http://127.0.0.1:7718/mcp`.

### Add `dobj` to your PATH

**macOS / Linux:**

```bash
echo 'export PATH="$HOME/.dobj/bin:$PATH"' >> ~/.zshrc   # or ~/.bashrc
```

**Windows** (new terminals pick it up):

```powershell
$bin = "$env:USERPROFILE\.dobj\bin"
$user = [Environment]::GetEnvironmentVariable("Path", "User")
if ($user -notlike "*$bin*") {
    [Environment]::SetEnvironmentVariable("Path", "$user;$bin", "User")
}
```

### Override the synchronizer / relayer endpoints

dobjd bakes hosted defaults in at build time — they're listed in the release
notes. Write `~/.dobj/settings.json` only to point somewhere else:

```json
{
  "synchronizerApiUrl": "http://...",
  "relayerApiUrl": "http://..."
}
```

(On Windows, write it BOM-less: `[System.IO.File]::WriteAllText(...)`, not
`Set-Content -Encoding utf8` on PowerShell 5.1.)

## Managing the daemon

| Command                      | Effect                                                    |
| ---------------------------- | --------------------------------------------------------- |
| `dobj start`                 | launch in the background (idempotent)                     |
| `dobj status`                | pid + HTTP healthcheck                                    |
| `dobj logs` / `dobj logs -f` | last 100 log lines / follow                               |
| `dobj stop`                  | shut down (SIGTERM→SIGKILL on Unix; hard kill on Windows) |
| `dobj update`                | upgrade to the latest release (plugins untouched)         |

Logs live at `~/.dobj/dobjd.log`.

## Manual install (no installer script)

The by-hand equivalent of `install.sh` / `install.ps1`:

**macOS / Linux:**

```bash
# 1. Detect your target triple
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64)   TARGET=aarch64-apple-darwin ;;
  Darwin-x86_64)  TARGET=x86_64-apple-darwin ;;
  Linux-x86_64)   TARGET=x86_64-unknown-linux-gnu ;;
  *) echo "unsupported platform: $(uname -sm)"; exit 1 ;;
esac

# 2. Download and extract the two artifacts. Substitute a pinned tag for
#    `latest/download` -> `download/<tag>` if you don't want the newest.
RELEASE=https://github.com/dobjlabs/digital-objects-network/releases/latest/download
mkdir -p ~/.dobj/bin
curl -fsSL "$RELEASE/dobjd-$TARGET.tar.gz" | tar -xz -C ~/.dobj/bin
curl -fsSL "$RELEASE/dobj-$TARGET.tar.gz"  | tar -xz -C ~/.dobj/bin
chmod +x ~/.dobj/bin/dobjd ~/.dobj/bin/dobj ~/.dobj/bin/dobj-mcp-proxy
```

**Windows (PowerShell):**

```powershell
$TARGET  = "x86_64-pc-windows-msvc"     # the only Windows target built
$RELEASE = "https://github.com/dobjlabs/digital-objects-network/releases/latest/download"
$DOBJ    = "$env:USERPROFILE\.dobj"
New-Item -ItemType Directory -Force -Path "$DOBJ\bin" | Out-Null
foreach ($name in @("dobjd", "dobj")) {
    $tmp = "$DOBJ\$name-$TARGET.tar.gz"
    curl.exe -fsSL "$RELEASE/$name-$TARGET.tar.gz" -o $tmp
    tar -xzf $tmp -C "$DOBJ\bin"
    Remove-Item $tmp
}
```

Then continue from [Start and verify](#start-and-verify).

## What's in a release

- **`dobjd-{target}.tar.gz`** — the daemon. Bundles `dobj-mcp-proxy` alongside.
- **`dobj-{target}.tar.gz`** — terminal CLI for the daemon.
- **`craft-basics.pexe`** and **`craft-rocket.pexe`** — example plugins
  (optional; see Next steps).

Plus `synchronizer-{target}.tar.gz` and `relayer-{target}.tar.gz` — server
binaries used by the install-test CI workflow. End users don't need these.

Targets: `aarch64-apple-darwin`, `x86_64-apple-darwin`,
`x86_64-unknown-linux-gnu`, `x86_64-pc-windows-msvc`.
