# bitcraft setup skill

Install the **bitcraft driver** on this machine. After this skill runs you'll have:

- `~/.dobj/bin/dobjd` (or `%USERPROFILE%\.dobj\bin\dobjd.exe` on Windows) — long-running driver process serving:
  - REST/SSE on `http://127.0.0.1:7717`
  - MCP on `http://127.0.0.1:7718/mcp`
- `~/.dobj/bin/dobj` — terminal CLI that talks to dobjd
- `~/.dobj/actions/episode-1.pexe` — the bundled crafting plugin
- `~/.dobj/settings.json` — pointing at the hosted synchronizer + relayer
- `~/.claude/skills/bitcraft-*/SKILL.md` — bitcraft commands (one per crafting class plus `create-command`)

bitcraft is a privacy-preserving crafting game where each item is a digital
object backed by a ZK proof. The driver runs locally, talks to a hosted
synchronizer + relayer for chain anchoring, and exposes the same API to
every client (CLI, agents via MCP, and optional desktop / web GUIs).

## Prerequisites

- macOS (arm64 or x86_64), Linux (x86_64), or Windows 10/11 (x86_64)
- macOS / Linux: `curl`, `tar`, `python3`, and a POSIX shell (preinstalled; plus `jq` if you'll register MCP with Claude Desktop in step 8)
- Windows: `curl.exe` + `tar.exe` (preinstalled on Windows 10 build 17063+) and PowerShell 5.1+ (preinstalled)

## Output rules

Your user-facing output is deterministic. This skill ALWAYS ends with a fresh install. The preflight may run an uninstall first as a prefix, but it never replaces the install.

Emit EXACTLY:

1. If the user confirms uninstall at preflight: one line per uninstall substep (`[U1/4] …` through `[U4/4] …`), then continue to step 1.
2. One line per install step (`[1/9] …` through `[9/9] …`) as each completes successfully.
3. The final success or failure block from "## Final output".
4. Verbatim tool error messages, only when a command fails.

The preflight prompt itself is the one exception to the "no other output" rule. Beyond that: no preamble, no greeting, no narration ("I'll start by..."), no commentary between steps ("that worked", "now I'll..."), no closing summary, no markdown bullets or headers around the per-step lines, no reflection.

Per-step line format (one line, plain text, output after the step succeeds):

```
[<n>/9] <label>
```

where `<label>` is exactly:

- 1: `detect platform`
- 2: `create dobj home`
- 3: `download dobjd + dobj`
- 4: `download episode-1.pexe`
- 5: `write settings.json`
- 6: `start dobjd`
- 7: `verify`
- 8: `register MCP with Claude Code`
- 9: `install bitcraft commands`

So a successful run prints exactly:

```
[1/9] detect platform
[2/9] create dobj home
[3/9] download dobjd + dobj
[4/9] download episode-1.pexe
[5/9] write settings.json
[6/9] start dobjd
[7/9] verify
[8/9] register MCP with Claude Code
[9/9] install bitcraft commands

<success block from "## Final output">
```

On step failure: skip remaining steps, print the failure block (with the failing step number and the verbatim error message), and stop.

## Preflight

Before step 1, check whether bitcraft is already installed:

```bash
[ -e ~/.dobj ] && echo "exists" || echo "fresh"
```

- **`fresh`** → skip the rest of this section and start at step 1.
- **`exists`** → prompt the user (use AskUserQuestion or whatever interactive prompt your agent supports):

  > An existing bitcraft install was detected at `~/.dobj`. Wipe it before reinstalling? Choosing yes removes everything under `~/.dobj` plus the bitcraft skills and MCP registration, then does a fresh install. Choosing no re-runs the install in place — every step is idempotent.
  - **No** → proceed directly to step 1.
  - **Yes** → run the uninstall substeps below, then proceed to step 1. (The install always runs; uninstall is just a prefix.)

### Uninstall substeps

Run in order. Each is best-effort — if a command fails (e.g. dobjd already stopped, `claude` not on PATH), continue.

#### U1. Stop dobjd

```bash
[ -x ~/.dobj/bin/dobj ] && ~/.dobj/bin/dobj stop || true
```

#### U2. Remove the bitcraft-preview entry from `~/.claude/launch.json`

The remover script lives inside the skills dir, so run it before U4 wipes the skills:

```bash
if [ -f ~/.claude/skills/bitcraft-start/ensure_launch.py ]; then
  python3 ~/.claude/skills/bitcraft-start/ensure_launch.py --remove || true
fi
```

#### U3. Unregister MCP with Claude Code

```bash
if command -v claude >/dev/null 2>&1; then
  claude mcp remove bitcraft 2>/dev/null || true
fi
```

For Claude Desktop, edit `~/Library/Application Support/Claude/claude_desktop_config.json` by hand and drop the `bitcraft` entry from `mcpServers` — there's no CLI for it.

#### U4. Remove `~/.dobj` and the skills shipped by this repo

Only remove the five skills bundled in `bitcraft-commands.tar.gz`. Any other `~/.claude/skills/bitcraft-*` directory is user-authored (via `create-command`) and must survive uninstall:

```bash
rm -rf ~/.dobj
for name in consult-docs create-command help preview start; do
  rm -rf "$HOME/.claude/skills/bitcraft-$name"
done
```

Per-substep line format (one line, plain text, output after the substep succeeds):

```
[U1/4] stop dobjd
[U2/4] remove launch.json entry
[U3/4] unregister MCP
[U4/4] remove ~/.dobj and bundled skills
```

After `[U4/4]`, continue to step 1.

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
RELEASE=https://bitcraft.s3.us-east-2.amazonaws.com/v0.1.0-rc.30
curl -fsSL "$RELEASE/dobjd-$TARGET.tar.gz" | tar -xz -C ~/.dobj/bin
curl -fsSL "$RELEASE/dobj-$TARGET.tar.gz"  | tar -xz -C ~/.dobj/bin
chmod +x ~/.dobj/bin/dobjd ~/.dobj/bin/dobj
```

**Windows:**

```powershell
# Use curl.exe explicitly — bare `curl` in PowerShell is an alias for
# Invoke-WebRequest with different flags.
$RELEASE = "https://bitcraft.s3.us-east-2.amazonaws.com/v0.1.0-rc.30"
$DOBJ = "$env:USERPROFILE\.dobj"
foreach ($name in @("dobjd", "dobj")) {
    $tmp = "$DOBJ\$name-$TARGET.tar.gz"
    curl.exe -fsSL "$RELEASE/$name-$TARGET.tar.gz" -o $tmp
    tar -xzf $tmp -C "$DOBJ\bin"
    Remove-Item $tmp
}
# No `chmod` — Windows runs anything with the `.exe` extension as executable.
```

### 4. Download the `episode-1` plugin

**macOS / Linux:**

```bash
curl -fsSL "$RELEASE/episode-1.pexe" \
  -o ~/.dobj/actions/episode-1.pexe
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
~/.dobj/bin/dobj actions     # confirms episode-1 plugin is loaded
~/.dobj/bin/dobj state-root  # confirms hosted synchronizer is reachable
```

**Windows:**

```powershell
& "$DOBJ\bin\dobj.exe" status
& "$DOBJ\bin\dobj.exe" actions
& "$DOBJ\bin\dobj.exe" state-root
```

You should see `dobjd is running (pid …)`, a long list of actions
(episode-1 ships ~75: `MineIron`, `CraftIngot`, `CraftSteel`,
`CraftRocket`, …), and a 64-character hex state root.

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

### 9. Install bitcraft commands as agent skills

The MCP server's instructions list bitcraft commands (`chop-log`,
`craft-wood`, `craft-sticks`, `craft-wood-pick`, `mine-stone`,
`craft-stone-pick`, `create-command`) by name only. The actual command
playbooks ship as Claude Code skills — one SKILL.md per command — that
the agent loads when the user invokes one.

```bash
SKILLS_DIR="$HOME/.claude/skills"
mkdir -p "$SKILLS_DIR"
curl -fsSL "$RELEASE/bitcraft-commands.tar.gz" | tar -xz -C "$SKILLS_DIR"
```

The tarball unpacks into `~/.claude/skills/bitcraft-<name>/SKILL.md`.

After it lands, fully restart Claude Code (close and reopen the chat)
so the new skills register.

**Claude Desktop and other agents** don't load `~/.claude/skills/`. For
those, the command names show up in the MCP instructions but the agent
has to follow the commands' bodies on its own — open the SKILL.md files
in `~/.claude/skills/bitcraft-*/` for the playbooks.

## Final output

Steps 1–9 complete the install. After step 9 succeeds, output the success
block below VERBATIM and stop. Do not run the sections that follow (those
are user reference, not install steps). No preamble. No closing line. No
commentary. No suggestions beyond what is in the block.

If any step 1–9 fails, output the failure block VERBATIM with the step
number and verbatim error message substituted, then stop.

### Success block

```
bitcraft is ready.

installed:
  ~/.dobj/bin/dobjd                     — driver daemon (HTTP :7717, MCP :7718)
  ~/.dobj/bin/dobj                      — terminal CLI
  ~/.dobj/actions/episode-1.pexe     — bundled crafting plugin
  ~/.dobj/settings.json                 — points at hosted synchronizer + relayer
  ~/.claude/skills/bitcraft-*/SKILL.md  — bitcraft commands

restart Claude Code, then type `bitcraft start` to begin a session.
```

### Failure block

Substitute `<n>` with the failing step number and `<error>` with the verbatim error message from the failed command (single line — strip newlines).

```
bitcraft install failed at step <n>: <error>

retry later, or ask in the bitcraft Discord: <discord-invite-url>
```

Replace `<discord-invite-url>` with the project's Discord invite when one is published; for now leave the literal placeholder so the user knows where to look.

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
