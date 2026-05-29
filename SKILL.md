# bitcraft setup skill

Install the **bitcraft driver** on this machine. After this skill runs you'll have:

- `~/.dobj/bin/dobjd` (or `%USERPROFILE%\.dobj\bin\dobjd.exe` on Windows) — long-running driver process serving:
  - REST/SSE on `http://127.0.0.1:7717`
  - MCP on `http://127.0.0.1:7718/mcp`
- `~/.dobj/bin/dobj` — terminal CLI that talks to dobjd
- `~/.dobj/actions/craft-basics.pexe` — the bundled crafting plugin
- `~/.dobj/settings.json` — pointing at the hosted synchronizer + relayer

bitcraft is a privacy-preserving crafting game where each item is a digital
object backed by a ZK proof. The driver runs locally, talks to a hosted
synchronizer + relayer for chain anchoring, and exposes the same API to
every client (CLI, agents via MCP, and optional desktop / web GUIs).

## Prerequisites

- macOS (arm64 or x86_64), Linux (x86_64), or Windows 10/11 (x86_64)
- macOS / Linux: `curl`, `tar`, and a POSIX shell (preinstalled)
- Windows: `curl.exe` + `tar.exe` (preinstalled on Windows 10 build 17063+) and PowerShell 5.1+ (preinstalled)

## Steps

Run these in order. Each is idempotent — safe to re-run.

**Pick a shell based on your OS** — every step has a bash variant for
macOS / Linux and a PowerShell variant for Windows. Run whichever matches.

### 1. Detect platform

**macOS / Linux (bash):**

```bash
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64)   TARGET=aarch64-apple-darwin ;;
  Darwin-x86_64)  TARGET=x86_64-apple-darwin ;;
  Linux-x86_64)   TARGET=x86_64-unknown-linux-gnu ;;
  *) echo "unsupported platform: $(uname -sm)"; exit 1 ;;
esac
echo "target: $TARGET"
```

**Windows (PowerShell):**

```powershell
# x86_64 is the only Windows target we currently build. Windows on arm64
# isn't supported yet — fail fast with a clear message if we're on one.
if ([Environment]::Is64BitOperatingSystem -and $env:PROCESSOR_ARCHITECTURE -ne "ARM64") {
    $TARGET = "x86_64-pc-windows-msvc"
} else {
    Write-Error "unsupported Windows architecture: $env:PROCESSOR_ARCHITECTURE (only x86_64 is built)"
    exit 1
}
Write-Host "target: $TARGET"
```

### 2. Create the dobj home

**macOS / Linux:**

```bash
mkdir -p ~/.dobj/bin ~/.dobj/actions
```

**Windows:**

```powershell
New-Item -ItemType Directory -Force -Path "$env:USERPROFILE\.dobj\bin", "$env:USERPROFILE\.dobj\actions" | Out-Null
```

### 3. Download `dobjd` and `dobj`

**macOS / Linux:**

```bash
RELEASE=https://bitcraft.s3.us-east-2.amazonaws.com/v0.1.0-rc.17
curl -fsSL "$RELEASE/dobjd-$TARGET.tar.gz" | tar -xz -C ~/.dobj/bin
curl -fsSL "$RELEASE/dobj-$TARGET.tar.gz"  | tar -xz -C ~/.dobj/bin
chmod +x ~/.dobj/bin/dobjd ~/.dobj/bin/dobj
```

**Windows:**

```powershell
# Use curl.exe explicitly — bare `curl` in PowerShell is an alias for
# Invoke-WebRequest with different flags.
$RELEASE = "https://bitcraft.s3.us-east-2.amazonaws.com/v0.1.0-rc.17"
$DOBJ = "$env:USERPROFILE\.dobj"
foreach ($name in @("dobjd", "dobj")) {
    $tmp = "$DOBJ\$name-$TARGET.tar.gz"
    curl.exe -fsSL "$RELEASE/$name-$TARGET.tar.gz" -o $tmp
    tar -xzf $tmp -C "$DOBJ\bin"
    Remove-Item $tmp
}
# No `chmod` — Windows runs anything with the `.exe` extension as executable.
```

### 4. Download the `craft-basics` plugin

**macOS / Linux:**

```bash
curl -fsSL "$RELEASE/craft-basics.pexe" \
  -o ~/.dobj/actions/craft-basics.pexe
```

**Windows:**

```powershell
curl.exe -fsSL "$RELEASE/craft-basics.pexe" -o "$DOBJ\actions\craft-basics.pexe"
```

### 5. Point the driver at the hosted synchronizer + relayer

**macOS / Linux:**

```bash
cat > ~/.dobj/settings.json <<'EOF'
{
  "synchronizerApiUrl": "http://18.217.144.33:3000",
  "relayerApiUrl": "http://18.217.144.33:3200"
}
EOF
```

**Windows:**

```powershell
# Write UTF-8 WITHOUT a BOM. Windows PowerShell 5.1's `Set-Content -Encoding utf8`
# prepends a BOM that dobjd's JSON parser rejects ("expected value at line 1");
# .NET's WriteAllText is BOM-less on both PowerShell 5.1 and 7.
$settings = @'
{
  "synchronizerApiUrl": "http://18.217.144.33:3000",
  "relayerApiUrl": "http://18.217.144.33:3200"
}
'@
[System.IO.File]::WriteAllText("$DOBJ\settings.json", $settings)
```

### 6. Start `dobjd` in the background

**macOS / Linux:**

```bash
~/.dobj/bin/dobj start
```

**Windows:**

```powershell
& "$DOBJ\bin\dobj.exe" start
```

Idempotent — if dobjd is already up, it just reports the existing pid. Logs
land at `~/.dobj/dobjd.log` (or `%USERPROFILE%\.dobj\dobjd.log`); pid at
the sibling `dobjd.pid`.

### 7. Verify

**macOS / Linux:**

```bash
~/.dobj/bin/dobj status      # pid + HTTP healthcheck
~/.dobj/bin/dobj actions     # confirms craft-basics plugin is loaded
~/.dobj/bin/dobj state-root  # confirms hosted synchronizer is reachable
```

**Windows:**

```powershell
& "$DOBJ\bin\dobj.exe" status
& "$DOBJ\bin\dobj.exe" actions
& "$DOBJ\bin\dobj.exe" state-root
```

You should see `dobjd is running (pid …)`, ~7 actions including `FindLog`
and `CraftWood`, and a 64-character hex state root.

### 8. Register MCP with the agent

dobjd exposes MCP at `http://127.0.0.1:7718/mcp` so this same agent can
drive bitcraft directly (list inventory, run actions, inspect classes).

If `claude` (Claude Code) is on the PATH, register the server now —
idempotent, safe to re-run:

**macOS / Linux:**

```bash
if command -v claude >/dev/null 2>&1; then
  claude mcp remove bitcraft 2>/dev/null || true
  claude mcp add --transport http bitcraft http://127.0.0.1:7718/mcp
fi
```

**Windows:**

```powershell
if (Get-Command claude -ErrorAction SilentlyContinue) {
    claude mcp remove bitcraft 2>$null
    claude mcp add --transport http bitcraft http://127.0.0.1:7718/mcp
}
```

The new MCP server takes effect on the next Claude Code session (close
and reopen the chat, or restart the CLI).

For **Claude Desktop** users, dobjd's HTTP MCP can't be registered
directly — Claude Desktop only speaks stdio. Use the bundled
`bitcraft-mcp-proxy` binary (installed alongside `dobjd` in step 3) as a
stdio↔HTTP bridge.

**macOS / Linux** — idempotent shell merge using `jq` (preinstalled on
most macOS machines; `brew install jq` / `apt install jq` otherwise) so
existing `mcpServers` entries aren't clobbered:

```bash
CONFIG="$HOME/Library/Application Support/Claude/claude_desktop_config.json"
mkdir -p "$(dirname "$CONFIG")"
[ -f "$CONFIG" ] || echo '{}' > "$CONFIG"
jq --arg cmd "$HOME/.dobj/bin/bitcraft-mcp-proxy" \
   '.mcpServers.bitcraft = {command: $cmd, args: ["--port", "7718"]}' \
   "$CONFIG" > "$CONFIG.tmp" && mv "$CONFIG.tmp" "$CONFIG"
```

**Windows** — same idea using built-in PowerShell JSON handling (no `jq`
needed). Claude Desktop reads `%APPDATA%\Claude\claude_desktop_config.json`:

```powershell
$config = "$env:APPDATA\Claude\claude_desktop_config.json"
New-Item -ItemType Directory -Force -Path (Split-Path $config -Parent) | Out-Null
if (-not (Test-Path $config)) { [System.IO.File]::WriteAllText($config, '{}') }

$json = Get-Content $config -Raw | ConvertFrom-Json
if (-not $json.PSObject.Properties.Match('mcpServers').Count) {
    $json | Add-Member -NotePropertyName mcpServers -NotePropertyValue ([pscustomobject]@{}) -Force
}
$json.mcpServers | Add-Member -NotePropertyName bitcraft -NotePropertyValue ([pscustomobject]@{
    command = "$env:USERPROFILE\.dobj\bin\bitcraft-mcp-proxy.exe"
    args    = @("--port", "7718")
}) -Force
# BOM-less UTF-8 (see settings.json note above) so Claude Desktop's JSON parser
# doesn't choke on a leading BOM and silently load zero MCP servers.
[System.IO.File]::WriteAllText($config, ($json | ConvertTo-Json -Depth 10))
```

Or hand-edit the config and merge with existing `mcpServers`:

- macOS: `~/Library/Application Support/Claude/claude_desktop_config.json`
- Windows: `%APPDATA%\Claude\claude_desktop_config.json`

```json
{
  "mcpServers": {
    "bitcraft": {
      "command": "<absolute path to bitcraft-mcp-proxy[.exe]>",
      "args": ["--port", "7718"]
    }
  }
}
```

Then fully quit Claude Desktop (Cmd+Q on macOS / right-click tray icon → Quit on Windows) and reopen.

For **other agents** (Cursor, Aider, Continue, etc.), point their MCP
configuration at `http://127.0.0.1:7718/mcp` via whatever UI / config
file they use.

## Optional: add `dobj` to your PATH

**macOS / Linux:**

```bash
echo 'export PATH="$HOME/.dobj/bin:$PATH"' >> ~/.zshrc
# or ~/.bashrc, depending on your shell
```

After restarting the shell you can drop the `~/.dobj/bin/` prefix from every
command.

**Windows** (appends to the user PATH — open a new terminal to pick it up):

```powershell
$bin = "$env:USERPROFILE\.dobj\bin"
$user = [Environment]::GetEnvironmentVariable("Path", "User")
if ($user -notlike "*$bin*") {
    [Environment]::SetEnvironmentVariable("Path", "$user;$bin", "User")
}
```

## Managing dobjd

**macOS / Linux:**

```bash
~/.dobj/bin/dobj start    # launch in background (idempotent)
~/.dobj/bin/dobj status   # is it running?
~/.dobj/bin/dobj logs     # last 100 lines of the log
~/.dobj/bin/dobj logs -f  # tail the log
~/.dobj/bin/dobj stop     # graceful shutdown (SIGTERM, SIGKILL fallback)
```

**Windows** (note: `stop` is a hard kill on Windows — there's no
graceful-signal equivalent, so the daemon exits immediately):

```powershell
& "$env:USERPROFILE\.dobj\bin\dobj.exe" start
& "$env:USERPROFILE\.dobj\bin\dobj.exe" status
& "$env:USERPROFILE\.dobj\bin\dobj.exe" logs
& "$env:USERPROFILE\.dobj\bin\dobj.exe" logs -f
& "$env:USERPROFILE\.dobj\bin\dobj.exe" stop
```
