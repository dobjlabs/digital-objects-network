#!/usr/bin/env bash
# Run all six bitcraft A2A agents locally, each pointing at its own dobjd
# (except the Auctioneer, which is pure routing — no dobjd).
#
#   Lumberjack-A   :9997   dobjd :7717   STICK_PRICE=5 satoshis
#   Lumberjack-B   :9995   dobjd :7767   STICK_PRICE=3 satoshis  ← wins
#   Stonemason     :9998   dobjd :7727
#   Craftsmith     :9999   dobjd :7737
#   Auctioneer     :9994   (no dobjd)
#   Concierge      :9996   dobjd :7747
#
# The Concierge calls the Auctioneer instead of going to a Lumberjack
# directly. The Auctioneer fans out to both Lumberjacks, picks the
# cheapest (Lumberjack-B at 3 satoshis), and forwards the result.
#
# Each URL is overridable via env: LUMBERJACK_URL, LUMBERJACK_BACKUP_URL,
# STONEMASON_URL, CRAFTSMITH_URL, AUCTIONEER_URL, CONCIERGE_URL.

set -euo pipefail
cd "$(dirname "$0")/.."

# Auto-load agents/.env if present. `set -a` makes every variable defined
# from here through `set +a` get exported, so anything sourced from .env
# is visible to child processes (mprocs, uv run, dobjd binaries…).
if [ -f .env ]; then
  set -a
  # shellcheck disable=SC1091
  . ./.env
  set +a
  echo ".env loaded"
fi

: "${LUMBERJACK_DOBJD:=http://127.0.0.1:7717}"
: "${LUMBERJACK_BACKUP_DOBJD:=http://127.0.0.1:7767}"
: "${STONEMASON_DOBJD:=http://127.0.0.1:7727}"
: "${CRAFTSMITH_DOBJD:=http://127.0.0.1:7737}"
: "${CONCIERGE_DOBJD:=http://127.0.0.1:7747}"

: "${LUMBERJACK_URL:=http://127.0.0.1:9997}"
: "${LUMBERJACK_BACKUP_URL:=http://127.0.0.1:9995}"
: "${STONEMASON_URL:=http://127.0.0.1:9998}"
: "${CRAFTSMITH_URL:=http://127.0.0.1:9999}"
: "${AUCTIONEER_URL:=http://127.0.0.1:9994}"
: "${CONCIERGE_URL:=http://127.0.0.1:9996}"

pids=()
cleanup() {
  echo
  echo "shutting down ${pids[*]}…"
  kill "${pids[@]}" 2>/dev/null || true
}
trap cleanup EXIT INT TERM

# ---- Lumberjacks ---------------------------------------------------------
echo "starting lumberjack-a on $LUMBERJACK_URL (dobjd=$LUMBERJACK_DOBJD, price=5)"
DOBJD_URL="$LUMBERJACK_DOBJD" \
A2A_PORT="${LUMBERJACK_URL##*:}" \
A2A_PUBLIC_URL="$LUMBERJACK_URL" \
AGENT_NAME='Lumberjack-A' \
STICK_PRICE=5 \
  uv run -m lumberjack &
pids+=($!)

echo "starting lumberjack-b on $LUMBERJACK_BACKUP_URL (dobjd=$LUMBERJACK_BACKUP_DOBJD, price=3)"
DOBJD_URL="$LUMBERJACK_BACKUP_DOBJD" \
A2A_PORT="${LUMBERJACK_BACKUP_URL##*:}" \
A2A_PUBLIC_URL="$LUMBERJACK_BACKUP_URL" \
AGENT_NAME='Lumberjack-B' \
STICK_PRICE=3 \
  uv run -m lumberjack &
pids+=($!)

# ---- Stonemason + Craftsmith ---------------------------------------------
echo "starting stonemason on $STONEMASON_URL (dobjd=$STONEMASON_DOBJD)"
DOBJD_URL="$STONEMASON_DOBJD" A2A_PORT="${STONEMASON_URL##*:}" \
A2A_PUBLIC_URL="$STONEMASON_URL" \
  uv run -m stonemason &
pids+=($!)

echo "starting craftsmith on $CRAFTSMITH_URL (dobjd=$CRAFTSMITH_DOBJD)"
DOBJD_URL="$CRAFTSMITH_DOBJD" A2A_PORT="${CRAFTSMITH_URL##*:}" \
A2A_PUBLIC_URL="$CRAFTSMITH_URL" \
  uv run -m craftsmith &
pids+=($!)

# ---- Auctioneer (no dobjd) -----------------------------------------------
# Brief sleep so the two Lumberjacks are likely up before the Auctioneer
# polls their agent cards on its first request. Not strictly required
# (Auctioneer would just fail one round if a peer is starting up), but
# avoids the noisy "bid failed" log line on a cold launch.
sleep 1

echo "starting auctioneer on $AUCTIONEER_URL (no dobjd)"
A2A_PORT="${AUCTIONEER_URL##*:}" \
A2A_PUBLIC_URL="$AUCTIONEER_URL" \
LUMBERJACK_URL="$LUMBERJACK_URL" \
LUMBERJACK_BACKUP_URL="$LUMBERJACK_BACKUP_URL" \
  uv run -m auctioneer &
pids+=($!)

# ---- Concierge -----------------------------------------------------------
sleep 1
echo "starting concierge on $CONCIERGE_URL (dobjd=$CONCIERGE_DOBJD)"
DOBJD_URL="$CONCIERGE_DOBJD" A2A_PORT="${CONCIERGE_URL##*:}" \
A2A_PUBLIC_URL="$CONCIERGE_URL" \
AUCTIONEER_URL="$AUCTIONEER_URL" \
STONEMASON_URL="$STONEMASON_URL" \
CRAFTSMITH_URL="$CRAFTSMITH_URL" \
  uv run -m concierge &
pids+=($!)

echo
echo "all six running."
echo "  concierge     $CONCIERGE_URL"
echo "  auctioneer    $AUCTIONEER_URL"
echo "  lumberjack-a  $LUMBERJACK_URL          (price=5)"
echo "  lumberjack-b  $LUMBERJACK_BACKUP_URL   (price=3)"
echo "  stonemason    $STONEMASON_URL"
echo "  craftsmith    $CRAFTSMITH_URL"
echo
echo "send a request:  uv run scripts/test_client.py"
echo "ctrl-C to stop all."
wait
