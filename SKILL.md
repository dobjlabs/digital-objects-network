# bitcraft setup skill

Install the **bitcraft driver** on this machine. After this skill runs you'll have:

- `~/.dobj/bin/dobjd` — long-running driver process serving:
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
RELEASE=https://github.com/dobjlabs/zk-craft/releases/latest/download
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
  "synchronizerApiUrl": "http://18.119.100.201:3000",
  "relayerApiUrl": "http://18.119.100.201:3200"
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

## Optional: register MCP with your agent

dobjd exposes MCP at `http://127.0.0.1:7718/mcp`. Add it via your agent's
normal MCP config:

```bash
# Claude Code:
claude mcp add bitcraft http://127.0.0.1:7718/mcp

# Other agents: paste http://127.0.0.1:7718/mcp into the MCP server field
# in your agent's settings.
```

## Optional: full chain round-trip

This produces a real on-chain transaction (one EIP-4844 blob via the
relayer). Run it to verify the end-to-end stack works:

```bash
~/.dobj/bin/dobj run FindLog
```

After it lands you'll see a `craft-basics_log_*.dobj` in `~/.dobj/objects/`.
The CLI streams progress events to stderr while it's running.

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
