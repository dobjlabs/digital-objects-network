#!/usr/bin/env bash
# Run all four bitcraft A2A agents locally, each pointing at its own dobjd.
#
# Each agent assumes its own dobjd at a distinct URL. Override via env:
#   LUMBERJACK_DOBJD, STONEMASON_DOBJD, CRAFTSMITH_DOBJD, CONCIERGE_DOBJD
#
# All four also need to know each other's A2A URLs — the concierge in
# particular. Defaults are localhost:9996-9999.

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
: "${STONEMASON_DOBJD:=http://127.0.0.1:7727}"
: "${CRAFTSMITH_DOBJD:=http://127.0.0.1:7737}"
: "${CONCIERGE_DOBJD:=http://127.0.0.1:7747}"

: "${LUMBERJACK_URL:=http://127.0.0.1:9997}"
: "${STONEMASON_URL:=http://127.0.0.1:9998}"
: "${CRAFTSMITH_URL:=http://127.0.0.1:9999}"
: "${CONCIERGE_URL:=http://127.0.0.1:9996}"

pids=()
cleanup() {
  echo
  echo "shutting down ${pids[*]}…"
  kill "${pids[@]}" 2>/dev/null || true
}
trap cleanup EXIT INT TERM

echo "starting lumberjack on $LUMBERJACK_URL (dobjd=$LUMBERJACK_DOBJD)"
DOBJD_URL="$LUMBERJACK_DOBJD" A2A_PORT="${LUMBERJACK_URL##*:}" \
  uv run -m lumberjack &
pids+=($!)

echo "starting stonemason on $STONEMASON_URL (dobjd=$STONEMASON_DOBJD)"
DOBJD_URL="$STONEMASON_DOBJD" A2A_PORT="${STONEMASON_URL##*:}" \
  uv run -m stonemason &
pids+=($!)

echo "starting craftsmith on $CRAFTSMITH_URL (dobjd=$CRAFTSMITH_DOBJD)"
DOBJD_URL="$CRAFTSMITH_DOBJD" A2A_PORT="${CRAFTSMITH_URL##*:}" \
  uv run -m craftsmith &
pids+=($!)

sleep 1

echo "starting concierge on $CONCIERGE_URL (dobjd=$CONCIERGE_DOBJD)"
DOBJD_URL="$CONCIERGE_DOBJD" A2A_PORT="${CONCIERGE_URL##*:}" \
  LUMBERJACK_URL="$LUMBERJACK_URL" \
  STONEMASON_URL="$STONEMASON_URL" \
  CRAFTSMITH_URL="$CRAFTSMITH_URL" \
  uv run -m concierge &
pids+=($!)

echo
echo "all four running."
echo "  concierge   $CONCIERGE_URL"
echo "  lumberjack  $LUMBERJACK_URL"
echo "  stonemason  $STONEMASON_URL"
echo "  craftsmith  $CRAFTSMITH_URL"
echo
echo "send a request:  uv run scripts/test_client.py"
echo "ctrl-C to stop all."
wait
