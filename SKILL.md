# bitcraft setup skill

Install the **bitcraft driver** on this machine. After this skill runs you'll have:

- `~/.dobj/bin/dobjd` — long-running driver process serving:
  - REST/SSE on `http://127.0.0.1:7717`
  - MCP on `http://127.0.0.1:7718/mcp`
- `~/.dobj/bin/dobj` — terminal CLI that talks to dobjd
- `~/.dobj/actions/craft-basics.pexe` — the bundled crafting plugin
- `~/.dobj/settings.json` — pointing at the hosted synchronizer + relayer
- `~/.claude/skills/bitcraft-*/SKILL.md` — bitcraft commands (one per crafting class plus `create-command`)

bitcraft is a privacy-preserving crafting game where each item is a digital
object backed by a ZK proof. The driver runs locally, talks to a hosted
synchronizer + relayer for chain anchoring, and exposes the same API to
every client (CLI, agents via MCP, and optional desktop / web GUIs).

## Prerequisites

- macOS (arm64 or x86_64) or Linux (x86_64)
- `curl`, `tar`, and a POSIX shell

## Steps

Run these in order. Each is idempotent — safe to re-run.

### 1. Detect platform

```bash
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64)   TARGET=aarch64-apple-darwin ;;
  Darwin-x86_64)  TARGET=x86_64-apple-darwin ;;
  Linux-x86_64)   TARGET=x86_64-unknown-linux-gnu ;;
  *) echo "unsupported platform: $(uname -sm)"; exit 1 ;;
esac
echo "target: $TARGET"
```

### 2. Create the dobj home

```bash
mkdir -p ~/.dobj/bin ~/.dobj/actions
```

### 3. Download `dobjd` and `dobj`

```bash
RELEASE=https://bitcraft.s3.us-east-2.amazonaws.com/v0.1.0-rc.17
curl -fsSL "$RELEASE/dobjd-$TARGET.tar.gz" | tar -xz -C ~/.dobj/bin
curl -fsSL "$RELEASE/dobj-$TARGET.tar.gz"  | tar -xz -C ~/.dobj/bin
chmod +x ~/.dobj/bin/dobjd ~/.dobj/bin/dobj
```

### 4. Download the `craft-basics` plugin

```bash
curl -fsSL "$RELEASE/craft-basics.pexe" \
  -o ~/.dobj/actions/craft-basics.pexe
```

### 5. Point the driver at the hosted synchronizer + relayer

```bash
cat > ~/.dobj/settings.json <<'EOF'
{
  "synchronizerApiUrl": "http://18.217.144.33:3000",
  "relayerApiUrl": "http://18.217.144.33:3200"
}
EOF
```

### 6. Start `dobjd` in the background

```bash
~/.dobj/bin/dobj start
```

Idempotent — if dobjd is already up, it just reports the existing pid. Logs
land at `~/.dobj/dobjd.log`; pid at `~/.dobj/dobjd.pid`.

### 7. Verify

```bash
~/.dobj/bin/dobj status      # pid + HTTP healthcheck
~/.dobj/bin/dobj actions     # confirms craft-basics plugin is loaded
~/.dobj/bin/dobj state-root  # confirms hosted synchronizer is reachable
```

You should see `dobjd is running (pid …)`, ~7 actions including `FindLog`
and `CraftWood`, and a 64-character hex state root.

### 8. Register MCP with the agent

dobjd exposes MCP at `http://127.0.0.1:7718/mcp` so this same agent can
drive bitcraft directly (list inventory, run actions, inspect classes).

If `claude` (Claude Code) is on the PATH, register the server now —
idempotent, safe to re-run:

```bash
if command -v claude >/dev/null 2>&1; then
  claude mcp remove bitcraft 2>/dev/null || true
  claude mcp add --transport http bitcraft http://127.0.0.1:7718/mcp
fi
```

The new MCP server takes effect on the next Claude Code session (close
and reopen the chat, or restart the CLI).

For **Claude Desktop** users, dobjd's HTTP MCP can't be registered
directly — Claude Desktop only speaks stdio. Use the bundled
`bitcraft-mcp-proxy` binary (installed alongside `dobjd` in step 3) as a
stdio↔HTTP bridge.

Idempotent shell merge — uses `jq` (preinstalled on most macOS machines;
`brew install jq` otherwise) so it doesn't clobber other MCP servers
already in the config:

```bash
CONFIG="$HOME/Library/Application Support/Claude/claude_desktop_config.json"
mkdir -p "$(dirname "$CONFIG")"
[ -f "$CONFIG" ] || echo '{}' > "$CONFIG"
jq --arg cmd "$HOME/.dobj/bin/bitcraft-mcp-proxy" \
   '.mcpServers.bitcraft = {command: $cmd, args: ["--port", "7718"]}' \
   "$CONFIG" > "$CONFIG.tmp" && mv "$CONFIG.tmp" "$CONFIG"
```

Or hand-edit `~/Library/Application Support/Claude/claude_desktop_config.json`
and merge with existing `mcpServers` (replace `<HOME>` with your home dir,
e.g. `/Users/alice`):

```json
{
  "mcpServers": {
    "bitcraft": {
      "command": "<HOME>/.dobj/bin/bitcraft-mcp-proxy",
      "args": ["--port", "7718"]
    }
  }
}
```

Then quit Claude Desktop fully (Cmd+Q) and reopen.

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

Register the compact-re-injection hook in `~/.claude/settings.json` so
that whenever Claude Code auto-compacts a conversation, the bitcraft
dispatch reminder + the live help block are re-emitted as fresh context.
The script is idempotent — safe to re-run, preserves any existing hooks
or settings:

```bash
python3 "$SKILLS_DIR/bitcraft-start/ensure_hook.py"
```

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
  ~/.dobj/actions/craft-basics.pexe     — bundled crafting plugin
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

```bash
echo 'export PATH="$HOME/.dobj/bin:$PATH"' >> ~/.zshrc
# or ~/.bashrc, depending on your shell
```

After restarting the shell you can drop the `~/.dobj/bin/` prefix from every
command.

## Managing dobjd

```bash
~/.dobj/bin/dobj start    # launch in background (idempotent)
~/.dobj/bin/dobj status   # is it running?
~/.dobj/bin/dobj logs     # last 100 lines of the log
~/.dobj/bin/dobj logs -f  # tail the log
~/.dobj/bin/dobj stop     # graceful shutdown (SIGTERM, SIGKILL fallback)
```
