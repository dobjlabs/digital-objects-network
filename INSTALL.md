# Installing bitcraft from a release

Manual install of the **bitcraft driver** from the prebuilt release artifacts.
This is the by-hand equivalent of the agent-driven [SKILL.md](SKILL.md) flow —
use it if you'd rather not hand the install to an MCP agent.

After this you'll have:

- `~/.dobj/bin/dobjd` (`dobjd.exe` on Windows) — the driver daemon, serving
  REST/SSE on `http://127.0.0.1:7717` and MCP on `http://127.0.0.1:7718/mcp`
- `~/.dobj/bin/dobj` — the terminal CLI
- `~/.dobj/bin/bitcraft-mcp-proxy` — stdio↔HTTP bridge for Claude Desktop
- `~/.dobj/actions/craft-basics.pexe` — the bundled crafting plugin

Only three artifacts are needed. The `synchronizer-*` and `relayer-*` tarballs
in the release are CI-only — a normal install talks to the hosted synchronizer

- relayer, whose URLs are baked into `dobjd` at build time.

## Prerequisites

- macOS (arm64 or x86_64), Linux (x86_64), or Windows 10/11 (x86_64)
- `tar` (preinstalled on macOS, Linux, and Windows 10 build 17063+)
- [`gh`](https://cli.github.com) (GitHub CLI), authenticated with access to
  `dobjlabs/zk-craft`. The repo and its releases are private, so `gh` handles
  the auth (and works even while a release is still a draft). Run
  `gh auth login` once if you haven't.

## macOS / Linux

```bash
# 1. Pick the release tag and detect your target triple
TAG=v0.1.0-rc.29                       # substitute the release you want
case "$(uname -sm)" in
  "Darwin arm64")  TARGET=aarch64-apple-darwin ;;
  "Darwin x86_64") TARGET=x86_64-apple-darwin ;;
  "Linux x86_64")  TARGET=x86_64-unknown-linux-gnu ;;
  *) echo "unsupported platform: $(uname -sm)"; exit 1 ;;
esac

# 2. Create the install home
mkdir -p ~/.dobj/bin ~/.dobj/actions

# 3. Download the three artifacts from the release
cd "$(mktemp -d)"
gh release download "$TAG" --repo dobjlabs/zk-craft \
  --pattern "dobjd-$TARGET.tar.gz" \
  --pattern "dobj-$TARGET.tar.gz" \
  --pattern "craft-basics*.pexe"

# 4. Extract into place
tar -xzf "dobjd-$TARGET.tar.gz" -C ~/.dobj/bin   # dobjd + bitcraft-mcp-proxy
tar -xzf "dobj-$TARGET.tar.gz"  -C ~/.dobj/bin   # dobj
cp craft-basics*.pexe ~/.dobj/actions/craft-basics.pexe
chmod +x ~/.dobj/bin/dobjd ~/.dobj/bin/dobj ~/.dobj/bin/bitcraft-mcp-proxy

# 5. (Optional) override the synchronizer/relayer endpoints. dobjd already
#    bakes in the hosted defaults at build time — write this only to point
#    somewhere else.
cat > ~/.dobj/settings.json <<'EOF'
{
  "synchronizerApiUrl": "https://sync.don.pateldhvani.com",
  "relayerApiUrl": "https://relay.don.pateldhvani.com"
}
EOF

# 6. Start and verify
~/.dobj/bin/dobj start         # first start builds ZK circuits — can take a few minutes
~/.dobj/bin/dobj status        # pid + HTTP healthcheck
~/.dobj/bin/dobj actions       # confirms craft-basics loaded (~7 actions)
~/.dobj/bin/dobj state-root    # confirms the synchronizer is reachable (64-hex root)
```

## Windows (PowerShell)

```powershell
# 1. Pick the release tag (x86_64 is the only Windows target built)
$TAG    = "v0.1.0-rc.30"               # substitute the release you want
$TARGET = "x86_64-pc-windows-msvc"
$DOBJ   = "$env:USERPROFILE\.dobj"

# 2. Create the install home
New-Item -ItemType Directory -Force -Path "$DOBJ\bin", "$DOBJ\actions" | Out-Null

# 3. Download the three artifacts from the release
$tmp = New-Item -ItemType Directory -Force -Path (Join-Path $env:TEMP "dobj-dl")
gh release download $TAG --repo dobjlabs/zk-craft `
  --pattern "dobjd-$TARGET.tar.gz" `
  --pattern "dobj-$TARGET.tar.gz" `
  --pattern "craft-basics*.pexe" `
  --dir $tmp

# 4. Extract into place
tar -xzf "$tmp\dobjd-$TARGET.tar.gz" -C "$DOBJ\bin"
tar -xzf "$tmp\dobj-$TARGET.tar.gz"  -C "$DOBJ\bin"
Copy-Item "$tmp\craft-basics*.pexe" "$DOBJ\actions\craft-basics.pexe"

# 5. (Optional) override endpoints — baked-in defaults are used otherwise.
@'
{
  "synchronizerApiUrl": "https://sync.don.pateldhvani.com",
  "relayerApiUrl": "https://relay.don.pateldhvani.com"
}
'@ | Set-Content -Path "$DOBJ\settings.json" -Encoding utf8

# 6. Start and verify
& "$DOBJ\bin\dobj.exe" start    # first start builds ZK circuits — can take a few minutes
& "$DOBJ\bin\dobj.exe" status
& "$DOBJ\bin\dobj.exe" actions
& "$DOBJ\bin\dobj.exe" state-root
```

**First-run note (Windows):** the binaries aren't codesigned yet, so Windows
SmartScreen may show "Windows protected your PC" → click **More info → Run
anyway**.

## Managing the daemon

| Command                      | Effect                                                    |
| ---------------------------- | --------------------------------------------------------- |
| `dobj start`                 | launch in the background (idempotent)                     |
| `dobj status`                | pid + HTTP healthcheck                                    |
| `dobj logs` / `dobj logs -f` | last 100 log lines / follow                               |
| `dobj stop`                  | shut down (SIGTERM→SIGKILL on Unix; hard kill on Windows) |

Prefix with `~/.dobj/bin/` (or `$DOBJ\bin\dobj.exe` on Windows), or add the
`bin` dir to your `PATH`. Logs live at `~/.dobj/dobjd.log`.

## Optional: connect an agent (MCP)

dobjd serves MCP at `http://127.0.0.1:7718/mcp`. Installing the binaries does
not register it with any agent — do that separately:

- **Claude Code:** `claude mcp add --transport http bitcraft http://127.0.0.1:7718/mcp`
- **Claude Desktop:** it only speaks stdio, so point it at the bundled
  `bitcraft-mcp-proxy` binary — see [SKILL.md](SKILL.md) step 8 for the exact
  `claude_desktop_config.json` entry.
- **Other agents** (Cursor, Aider, …): point their MCP config at
  `http://127.0.0.1:7718/mcp`.

## Notes

- The example tag above (`v0.1.0-rc.30`) is illustrative — substitute the
  release you want. `gh release list --repo dobjlabs/zk-craft` shows what's
  available.
- `gh release download` is used because the repo is private. If you publish
  artifacts to a public mirror for end users, swap step 3 for a plain
  `curl` against that mirror (this is what [SKILL.md](SKILL.md) does).
