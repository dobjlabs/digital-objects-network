#!/usr/bin/env bash
# Quick health summary of the six bootstrapped dobjds.
#
# Hits each one's /healthz, /state-root, /inventory, /actions, /classes
# and prints a one-line summary per agent. Use after bootstrap_dobjds.sh
# is up to confirm all six are healthy and talking to the synchronizer.

set -uo pipefail

declare -a ROWS=(
  "lumberjack:7727"
  "stonemason:7737"
  "craftsmith:7747"
  "concierge:7757"
  "lumberjack_b:7767"
  "auctioneer:7777"
)

fail=0
printf "%-14s %-7s %-7s %-9s %-9s %s\n" \
  "agent" "http" "health" "inv" "actions" "state-root"
printf "%-14s %-7s %-7s %-9s %-9s %s\n" \
  "-----" "----" "------" "---" "-------" "----------"

for entry in "${ROWS[@]}"; do
  name="${entry%:*}"
  port="${entry#*:}"
  base="http://127.0.0.1:$port"

  health=$(curl -fsS --max-time 2 "$base/healthz" 2>/dev/null || echo "DOWN")
  if [ "$health" = "DOWN" ]; then
    printf "%-14s %-7s %-7s %-9s %-9s %s\n" \
      "$name" "$port" "DOWN" "-" "-" "-"
    fail=1
    continue
  fi

  inv_count=$(curl -fsS --max-time 5 "$base/inventory" 2>/dev/null \
    | jq 'length' 2>/dev/null || echo "?")
  actions_count=$(curl -fsS --max-time 5 "$base/actions" 2>/dev/null \
    | jq 'length' 2>/dev/null || echo "?")
  # /state-root returns a bare JSON string like "0xabc…" — extract with `jq -r .`
  state_root=$(curl -fsS --max-time 5 "$base/state-root" 2>/dev/null \
    | jq -r '.' 2>/dev/null || echo "?")
  state_root_short="${state_root:0:18}…"

  printf "%-12s %-7s %-7s %-9s %-9s %s\n" \
    "$name" "$port" "ok" "$inv_count" "$actions_count" "$state_root_short"
done

exit $fail
