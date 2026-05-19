#!/usr/bin/env bash
# Entrypoint for one bitcraft A2A agent + colocated dobjd, in a single
# container. Used by both `docker run` and Render.
#
# Required env:
#   AGENT          one of: lumberjack | stonemason | craftsmith | concierge
#   PORT           public A2A port (Render injects; default 9996)
#   ANTHROPIC_API_KEY (or whichever provider LLM_MODEL points at)
#
# Peer URLs (only the Concierge consults these; specialists ignore them):
#   LUMBERJACK_URL   e.g. https://lumberjack-xxx.onrender.com
#   STONEMASON_URL
#   CRAFTSMITH_URL
#
# Optional:
#   DOBJD_PORT       internal dobjd HTTP port (default 7717; MCP is +1)
#   SYNC_URL         synchronizer URL (default: hosted)
#   RELAY_URL        relayer URL (default: hosted)
#   LLM_MODEL        LiteLLM model string (default anthropic/claude-opus-4-7)

set -euo pipefail

AGENT="${AGENT:?set AGENT to lumberjack|stonemason|craftsmith|concierge}"
PORT="${PORT:-9996}"
DOBJD_PORT="${DOBJD_PORT:-7717}"
SYNC_URL="${SYNC_URL:-http://18.217.144.33:3000}"
RELAY_URL="${RELAY_URL:-http://18.217.144.33:3200}"

# ---------------------------------------------------------------------------
# 1. Per-agent ~/.dobj scaffolding
# ---------------------------------------------------------------------------
# Each container is one agent, so a single ~/.dobj suffices (unlike the
# local `bootstrap_dobjds.sh` which fakes four $HOMEs on one machine).
DOBJ_HOME="${HOME:-/root}/.dobj"
mkdir -p "$DOBJ_HOME/actions" "$DOBJ_HOME/objects"

# Mount-friendly: only write settings.json if it doesn't already exist
# (so a Render persistent disk can hold custom values across restarts).
if [ ! -f "$DOBJ_HOME/settings.json" ]; then
    cat > "$DOBJ_HOME/settings.json" <<EOF
{
  "synchronizerApiUrl": "$SYNC_URL",
  "relayerApiUrl": "$RELAY_URL"
}
EOF
    echo "[entrypoint] wrote $DOBJ_HOME/settings.json (sync=$SYNC_URL relay=$RELAY_URL)"
fi

# Plugin: symlink in (so a future `just install-plugins` rebuild only
# needs to update one file, not stomp every container).
if [ ! -e "$DOBJ_HOME/actions/craft-basics.pexe" ]; then
    ln -sf /app/dobjd/actions/craft-basics.pexe "$DOBJ_HOME/actions/craft-basics.pexe"
    echo "[entrypoint] linked craft-basics.pexe into $DOBJ_HOME/actions/"
fi

# ---------------------------------------------------------------------------
# 2. Launch dobjd in the background
# ---------------------------------------------------------------------------
echo "[entrypoint] starting dobjd on :$DOBJD_PORT (MCP :$((DOBJD_PORT + 1)))"
DOBJD_PORT="$DOBJD_PORT" \
RUST_LOG="${RUST_LOG:-info}" \
    /app/dobjd/dobjd >/tmp/dobjd.log 2>&1 &
DOBJD_PID=$!

# Forward dobjd logs into our own stdout so Render captures them.
# `tail -f` runs in a subshell that dies with us thanks to the trap below.
( tail -F /tmp/dobjd.log 2>/dev/null | sed 's/^/[dobjd] /' ) &
TAIL_PID=$!

cleanup() {
    echo "[entrypoint] shutting down…"
    kill "$DOBJD_PID" 2>/dev/null || true
    kill "$TAIL_PID" 2>/dev/null || true
    wait 2>/dev/null || true
}
trap cleanup EXIT INT TERM

# Wait for dobjd to come up before launching the agent. The agent fails
# fast if dobjd isn't reachable, so polling here gives us a tighter loop
# and a clearer error message.
echo "[entrypoint] polling dobjd /healthz…"
for i in $(seq 1 60); do
    if curl -fsS --max-time 1 "http://127.0.0.1:$DOBJD_PORT/healthz" >/dev/null 2>&1; then
        echo "[entrypoint] dobjd ready after ${i}s"
        break
    fi
    if ! kill -0 "$DOBJD_PID" 2>/dev/null; then
        echo "[entrypoint] dobjd died before becoming ready — last 30 lines:" >&2
        tail -n 30 /tmp/dobjd.log >&2 || true
        exit 1
    fi
    sleep 1
done
if ! curl -fsS --max-time 1 "http://127.0.0.1:$DOBJD_PORT/healthz" >/dev/null 2>&1; then
    echo "[entrypoint] dobjd never became healthy — last 30 lines:" >&2
    tail -n 30 /tmp/dobjd.log >&2 || true
    exit 1
fi

# ---------------------------------------------------------------------------
# 3. Launch the A2A agent in the foreground
# ---------------------------------------------------------------------------
# A2A_PUBLIC_URL drives what the agent card advertises as its own URL.
# Render injects RENDER_EXTERNAL_URL; outside Render the caller should
# set A2A_PUBLIC_URL explicitly.
export A2A_HOST="${A2A_HOST:-0.0.0.0}"
export A2A_PORT="$PORT"
export A2A_PUBLIC_URL="${A2A_PUBLIC_URL:-${RENDER_EXTERNAL_URL:-http://127.0.0.1:$PORT}}"
export DOBJD_URL="http://127.0.0.1:$DOBJD_PORT"

echo "[entrypoint] launching agent=$AGENT a2a=$A2A_PORT public=$A2A_PUBLIC_URL"
cd /app/agents
exec python -m "$AGENT"
