# Digital Objects setup skill

Set up the **Digital Objects driver** on this machine. After this runs you'll have:

- `~/.dobj/bin/dobjd` (`dobjd.exe` on Windows) - the driver daemon, serving:
  - REST/SSE on `http://127.0.0.1:7717`
  - MCP on `http://127.0.0.1:7718/mcp` (off by default; step 5 turns it on)
- `~/.dobj/bin/dobj` - terminal CLI that talks to dobjd
- `~/.dobj/bin/dobj-mcp-proxy` - stdio<->HTTP bridge for Claude Desktop
- `~/.dobj/actions/craft-basics.pexe` - the bundled crafting plugin

Digital Objects is a network for privately-held, ZK-proved stateful objects;
the bundled `craft-basics` plugin is a small crafting demo where each item is
one such object. The driver runs locally, talks to a hosted synchronizer +
relayer for chain anchoring (their URLs are baked into the binaries at build
time), and exposes the same API to every client (CLI, agents via MCP, and
optional desktop / web GUIs).

## Prerequisites

- macOS (arm64 or x86_64), Linux (x86_64), or Windows 10/11 (x86_64)
- macOS / Linux: `curl` and a POSIX shell (preinstalled)
- Windows: PowerShell 5.1+, plus `curl.exe` + `tar.exe` (preinstalled on
  Windows 10 build 17063+)

## Steps

Run these in order. Each is idempotent - safe to re-run. Pick the shell that
matches the OS: a bash variant for macOS / Linux, a PowerShell variant for
Windows.

### 1. Install the binaries

Downloads the latest release of `dobjd`, `dobj`, and `dobj-mcp-proxy`
from the project's GitHub releases into `~/.dobj/bin`. The installer detects the
platform and prints a PATH hint. To pin a version, set `DOBJ_VERSION` (bash)
/ `$env:DOBJ_VERSION` (PowerShell) to a release tag first.

**macOS / Linux:**

```bash
curl -fsSL https://raw.githubusercontent.com/dobjlabs/digital-objects-network/main/install.sh | sh
```

**Windows (PowerShell):**

```powershell
irm https://raw.githubusercontent.com/dobjlabs/digital-objects-network/main/install.ps1 | iex
```

### 2. Start the daemon

The first start builds ZK circuits and can take a few minutes; later starts
are seconds. Idempotent - if dobjd is already up, it reports the existing pid.

**macOS / Linux:**

```bash
~/.dobj/bin/dobj start
```

**Windows (PowerShell):**

```powershell
& "$env:USERPROFILE\.dobj\bin\dobj.exe" start
```

### 3. Install the craft-basics plugin

The driver starts with an empty action catalog; the crafting demo's actions
ship as the `craft-basics` plugin. `dobj install` downloads the `.pexe`,
validates it, and hot-reloads the running daemon's catalog - no restart
needed. Re-running replaces the installed copy.

**macOS / Linux:**

```bash
~/.dobj/bin/dobj install https://github.com/dobjlabs/digital-objects-network/releases/latest/download/craft-basics.pexe
```

**Windows (PowerShell):**

```powershell
& "$env:USERPROFILE\.dobj\bin\dobj.exe" install https://github.com/dobjlabs/digital-objects-network/releases/latest/download/craft-basics.pexe
```

### 4. Verify

**macOS / Linux:**

```bash
~/.dobj/bin/dobj status      # pid + HTTP healthcheck
~/.dobj/bin/dobj actions     # 9 actions, e.g. FindLog, CraftWood
~/.dobj/bin/dobj state-root  # 64-hex root: hosted synchronizer reachable
```

**Windows (PowerShell):**

```powershell
& "$env:USERPROFILE\.dobj\bin\dobj.exe" status
& "$env:USERPROFILE\.dobj\bin\dobj.exe" actions
& "$env:USERPROFILE\.dobj\bin\dobj.exe" state-root
```

You should see `dobjd is running (pid ...)`, the craft-basics actions, and a
64-character hex state root.

### 5. Register MCP with the agent

dobjd can serve MCP at `http://127.0.0.1:7718/mcp` so this agent can drive
Digital Objects directly (list objects, run actions, inspect classes).

MCP is **off by default**, so first turn it on. This takes effect
immediately - no daemon restart - and persists across restarts.

macOS / Linux:

```bash
~/.dobj/bin/dobj settings set --mcp on
```

Windows (PowerShell):

```powershell
& "$env:USERPROFILE\.dobj\bin\dobj.exe" settings set --mcp on
```

Then register the MCP client.

**Claude Code** - if `claude` is on the PATH, register it now (idempotent):

macOS / Linux:

```bash
if command -v claude >/dev/null 2>&1; then
  claude mcp remove dobj 2>/dev/null || true
  claude mcp add --transport http dobj http://127.0.0.1:7718/mcp
fi
```

Windows (PowerShell):

```powershell
if (Get-Command claude -ErrorAction SilentlyContinue) {
    claude mcp remove dobj 2>$null
    claude mcp add --transport http dobj http://127.0.0.1:7718/mcp
}
```

The new server takes effect on the next Claude Code session (restart the CLI
or open a new chat).

**Claude Desktop** only speaks stdio, so point it at the bundled
`dobj-mcp-proxy`. Merge a `dobj` entry into `mcpServers` in its
config, preserving any existing servers.

macOS / Linux (uses `jq`; `brew install jq` / `apt install jq` if missing):

```bash
CONFIG="$HOME/Library/Application Support/Claude/claude_desktop_config.json"
mkdir -p "$(dirname "$CONFIG")"
[ -f "$CONFIG" ] || echo '{}' > "$CONFIG"
jq --arg cmd "$HOME/.dobj/bin/dobj-mcp-proxy" \
   '.mcpServers.dobj = {command: $cmd, args: ["--port", "7718"]}' \
   "$CONFIG" > "$CONFIG.tmp" && mv "$CONFIG.tmp" "$CONFIG"
```

Windows (PowerShell) - reads `%APPDATA%\Claude\claude_desktop_config.json`:

```powershell
$config = "$env:APPDATA\Claude\claude_desktop_config.json"
New-Item -ItemType Directory -Force -Path (Split-Path $config -Parent) | Out-Null
if (-not (Test-Path $config)) { [System.IO.File]::WriteAllText($config, '{}') }
$json = Get-Content $config -Raw | ConvertFrom-Json
if (-not $json.PSObject.Properties.Match('mcpServers').Count) {
    $json | Add-Member -NotePropertyName mcpServers -NotePropertyValue ([pscustomobject]@{}) -Force
}
$json.mcpServers | Add-Member -NotePropertyName dobj -NotePropertyValue ([pscustomobject]@{
    command = "$env:USERPROFILE\.dobj\bin\dobj-mcp-proxy.exe"
    args    = @("--port", "7718")
}) -Force
# BOM-less UTF-8 so Claude Desktop's JSON parser doesn't choke on a leading BOM.
[System.IO.File]::WriteAllText($config, ($json | ConvertTo-Json -Depth 10))
```

Then fully quit Claude Desktop (Cmd+Q on macOS / Quit from the tray on
Windows) and reopen.

For **other agents** (Cursor, Aider, Continue, ...), point their MCP config
at `http://127.0.0.1:7718/mcp`.

### 6. Try it

With the MCP connected, drive Digital Objects in plain language - or use the
built-in command UX. In Claude Code, run `/mcp__dobj__start` (or just type
`start`) to open it; other clients invoke the `start` prompt the same way. From
there: `help` lists the commands, `create-command` defines your own, `dashboard`
opens a live view, and `consult-docs` answers questions from the reference docs.

## Managing the daemon

Prefix with `~/.dobj/bin/`, or add that directory to your PATH (the
installer prints the exact line).

```bash
dobj start    # launch in the background (idempotent)
dobj status   # is it running?
dobj logs -f  # tail the log at ~/.dobj/dobjd.log
dobj stop     # graceful shutdown (hard kill on Windows)
dobj update   # update dobj + dobjd + dobj-mcp-proxy to the latest release
```

On Windows, invoke as `& "$env:USERPROFILE\.dobj\bin\dobj.exe" <cmd>`.

`dobj update` leaves plugins under `~/.dobj/actions/` untouched. `dobj start`
and `dobj status` print a one-line notice when a newer release is available.
