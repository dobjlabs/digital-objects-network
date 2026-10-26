#!/usr/bin/env bash
# Print a slot the synchronizer can use as its genesis slot (INIT_START_SLOT): one that is
# finalized and has an execution payload that became canonical with it.
#
#   utils/genesis-slot.sh <beacon_url>
#
# Walks back from the finalized slot to the first such slot, using the synchronizer's rule: up to
# Fulu a block's embedded payload is canonical; from Gloas the parent's bid payload is, when this
# block's bid builds on it. Exits non-zero if none is found within 64 slots.
set -uo pipefail

if [ $# -ne 1 ]; then
    echo "usage: $0 <beacon_url>" >&2
    exit 2
fi
beacon="${1%/}"
slot="$(curl -fsSL "${beacon}/eth/v1/beacon/headers/finalized" | jq -r '.data.header.message.slot')" || exit 1
for _ in $(seq 64); do
    [[ "$slot" =~ ^[0-9]+$ ]] && [ "$slot" -ge 1 ] || exit 1
    if block="$(curl -fsSL "${beacon}/eth/v2/beacon/blocks/${slot}" 2>/dev/null)"; then
        if [ "$(jq -r '.data.message.body.execution_payload != null' <<<"$block")" = true ]; then
            echo "$slot"; exit 0
        fi
        parent_root="$(jq -r '.data.message.parent_root' <<<"$block")"
        built_on="$(jq -r '.data.message.body.signed_execution_payload_bid.message.parent_block_hash' <<<"$block")"
        parent_payload="$(curl -fsSL "${beacon}/eth/v2/beacon/blocks/${parent_root}" \
            | jq -r '.data.message.body.signed_execution_payload_bid.message.block_hash // empty')" || exit 1
        if [ -n "$parent_payload" ] && [ "$parent_payload" = "$built_on" ]; then
            echo "$slot"; exit 0
        fi
    fi
    slot=$((slot - 1))
done
exit 1
