# Digital Objects Network justfile
# Install just: https://github.com/casey/just

# Run the synchronizer (loads env from services/synchronizer/.env if present)
sync:
    RUST_LOG=info cargo run -p synchronizer --release

# Run the relayer (loads env from services/relayer/.env if present)
relayer:
    RUST_LOG=info cargo run -p relayer --release

# Run the archiver (loads env from services/archiver/.env if present)
archiver:
    RUST_LOG=info cargo run -p archiver --release

# The 2s block time is load-bearing, and the state file must stay in step with
# the archiver + synchronizer stores; see devtools/beacon-shim/README.md.
# Run the local anvil devnet that backs `just dev-local`.
anvil:
    @mkdir -p data
    anvil --block-time 2 --port 8545 --state data/anvil-state.json --state-interval 5

# Run the Beacon REST shim that projects anvil blocks onto beacon slots for the
# archiver and synchronizer. Waits for anvil, so start order does not matter.
beacon-shim:
    RUST_LOG=info cargo run -p beacon-shim --release

# Run the desktop app standalone (Tauri spawns its own Vite on :1420).
# Use this when you only want the desktop window. Inside `just dev` we use
# `desktop-shell` instead so a shared Vite serves both desktop and browser.
desktop:
    cd interfaces/gui && RUST_BACKTRACE=1 RUST_LOG=info pnpm tauri dev --release

# Run the Tauri shell pointing at an *already-running* Vite at :1420.
# Skips Tauri's `beforeDevCommand` so it doesn't fight the standalone web
# pane for the port. Pair with `just web`.
desktop-shell:
    cd interfaces/gui && RUST_LOG=info pnpm tauri dev --release -c '{"build":{"beforeDevCommand":""}}'

# Run the Vite dev server alone on :1420. Reachable from any browser tab
# or from the Tauri shell. Talks to dobjd at :7717 over HTTP for everything
# driver-related.
web:
    cd interfaces/gui && pnpm install && pnpm dev

# Run the documentation site (Vocs dev server) with hot reload
docs:
    cd docs && pnpm install && pnpm dev

# Run the headless HTTP server that exposes the driver API to every client
# (desktop window, browser tab, MCP, dobj CLI).
dobjd:
    RUST_LOG=info cargo run -p dobjd --release

# Bring up everything: synchronizer, relayer, dobjd, Vite, and the Tauri
# shell — all backed by one dobjd process. Open http://localhost:1420 in a
# browser to use the website client; the desktop window opens automatically.
# https://github.com/pvolok/mprocs
dev: ensure-db ensure-start-slot ensure-plugins ensure-mcp ensure-mcp-enabled
    mprocs --config mprocs.yaml

# Like `just dev`, but without spawning the local synchronizer + relayer —
# point dobjd at the hosted public endpoints instead. Faster spin-up when
# you don't need to fork the chain locally and don't want a local Postgres.
# Uses the standard 7717 default (same as `just dev`).
dev-remote: ensure-remote-settings ensure-plugins ensure-mcp ensure-mcp-enabled
    mprocs --config mprocs.remote.yaml

# Like `just dev`, but against a local anvil devnet instead of a public
# Ethereum endpoint, so nothing in the stack reaches the network. The chain
# vars are exported rather than written to each service's .env: dotenvy leaves
# an already-set variable alone, so the environment wins and the .env files
# stay untouched for `just dev`.
#
# Stores are kept separate from `just dev`'s (own Postgres databases, own
# RocksDB path, own blobs directory) because the two point at different chains
# and sharing them would leave the synchronizer deriving against roots that
# never existed on the other one. See devtools/beacon-shim/README.md.
dev-local: ensure-anvil ensure-db-local ensure-local-settings ensure-plugins ensure-mcp ensure-mcp-enabled
    #!/usr/bin/env bash
    set -euo pipefail
    export RPC_URL=http://127.0.0.1:8545
    export BEACON_URL=http://127.0.0.1:8555
    export ARCHIVER_URL=http://127.0.0.1:3001
    export TO_ADDRESS=0x4343434343434343434343434343434343434343
    export FILTER_ADDRESS="$TO_ADDRESS"
    # anvil's first genesis-funded dev account. Published test key, local only.
    export PRIVATE_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80
    # Must be >= 1: the synchronizer bootstraps from INIT_START_SLOT - 1.
    export INIT_START_SLOT=1
    export BLOBS_PATH=data/blobs-local/
    export APP_STATE_DB_PATH=data/synchronizer-db-local
    export SYNC_METADATA_DB_URL=postgres://postgres@localhost:5432/synchronizer_local
    export DB_URL=postgres://postgres@localhost:5432/relayer_local
    # The shipped defaults pace requests for a 15 req/s public endpoint.
    export SYNC_DELAY_MS=50
    export CATCHUP_BATCH_SIZE=64
    mprocs --config mprocs.local.yaml

# Fail with the install command when anvil is missing. Unlike the optional
# `claude` CLI in ensure-mcp, `just dev-local` cannot run without it.
ensure-anvil:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! command -v anvil >/dev/null 2>&1; then
        echo "anvil not found; install foundry with:" >&2
        echo "    curl -L https://foundry.paradigm.xyz | bash && foundryup" >&2
        exit 1
    fi
    echo "anvil: $(anvil --version | head -n1)"

# Create the Postgres databases `just dev-local` uses. Separate from
# `ensure-db` so the devnet and the public-endpoint stack never share a store.
ensure-db-local: (create-db "synchronizer_local") (create-db "relayer_local")

# Point ~/.dobj/settings.json at the local synchronizer + relayer. The
# counterpart to `ensure-remote-settings`, which is sticky: without this a
# `just dev-remote` leaves dobjd talking to the hosted services, so a later
# `just dev-local` would prove against them while anvil runs untouched.
ensure-local-settings:
    #!/usr/bin/env bash
    set -euo pipefail
    f="$HOME/.dobj/settings.json"
    mkdir -p "$HOME/.dobj"
    cur="{}"; [ -f "$f" ] && cur="$(jq '.' "$f" 2>/dev/null || echo '{}')"
    printf '%s' "$cur" | jq '. + {synchronizerApiUrl:"http://127.0.0.1:3000", relayerApiUrl:"http://127.0.0.1:3200"}' > "$f.tmp"
    mv "$f.tmp" "$f"
    echo "~/.dobj/settings.json -> local sync + relayer"

# Block (up to ~5 min) until an HTTP endpoint responds, then return. mprocs
# uses this to launch synchronizer -> relayer -> dobjd -> web -> desktop in
# order, each gated on the previous one's health, so they don't race to
# cold-build the shared proving-circuit cache on first run.
wait-health URL:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "waiting for {{URL}} ..."
    for _ in $(seq 1 600); do
        if curl -sf "{{URL}}" >/dev/null 2>&1; then
            echo "{{URL}} is up"
            exit 0
        fi
        sleep 0.5
    done
    echo "timed out waiting for {{URL}}; starting anyway"

# Idempotently point ~/.dobj/settings.json at the hosted synchronizer + relayer
ensure-remote-settings:
    @mkdir -p ~/.dobj
    @printf '{"synchronizerApiUrl":"https://synchronizer.don.pateldhvani.com","relayerApiUrl":"https://relayer.don.pateldhvani.com"}\n' > ~/.dobj/settings.json
    @echo "~/.dobj/settings.json → hosted sync + relayer"

# Install plugins into ~/.dobj/actions/ if none are present. Runs as part of
# `just dev` so a fresh clone (or a `just reset`-ed dev env) boots cleanly.
ensure-plugins:
    @mkdir -p ~/.dobj/actions
    @if [ -z "$(find ~/.dobj/actions -maxdepth 1 -name '*.pexe' -print -quit)" ]; then \
        echo "No .pexe plugins installed — packaging from examples/ and installing..."; \
        just install-plugins; \
    fi

# Register the dobj MCP with Claude Code at project (default) scope, so it
# only loads in chats started from this directory. Other directories stay
# uncontaminated by the dobj dispatch rules. Idempotent: remove + add on
# each run so the URL stays current. Skipped silently if the `claude` CLI is
# missing.
ensure-mcp:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! command -v claude >/dev/null 2>&1; then
        exit 0
    fi
    claude mcp remove dobj 2>/dev/null || true
    claude mcp add --transport http dobj http://127.0.0.1:7718/mcp \
        && echo "registered: dobj MCP (project scope, http://127.0.0.1:7718/mcp)"

# Force the daemon's MCP toggle on for `just dev`. `mcpEnabled` is a persisted
# setting that defaults off, so without this a fresh ~/.dobj would boot with MCP
# disabled. Read-modify-write: keep any existing synchronizer/relayer URLs (both
# are required fields, so the file must stay complete) and seed the local-dev
# defaults when the file is absent. Idempotent.
ensure-mcp-enabled:
    #!/usr/bin/env bash
    set -euo pipefail
    f="$HOME/.dobj/settings.json"
    mkdir -p "$HOME/.dobj"
    cur="{}"; [ -f "$f" ] && cur="$(jq '.' "$f" 2>/dev/null || echo '{}')"
    printf '%s' "$cur" | jq '{synchronizerApiUrl:"http://127.0.0.1:3000", relayerApiUrl:"http://127.0.0.1:3200"} + . + {mcpEnabled:true}' > "$f.tmp"
    mv "$f.tmp" "$f"
    echo "~/.dobj/settings.json -> mcpEnabled=true"

# Create one Postgres database if it is absent. Idempotent.
create-db NAME:
    @psql postgres://postgres@localhost:5432/postgres -tAc "SELECT 1 FROM pg_database WHERE datname='{{NAME}}'" | grep -q 1 || psql postgres://postgres@localhost:5432/postgres -c 'CREATE DATABASE {{NAME}}'

# Ensure the local Postgres databases the synchronizer + relayer expect exist.
# `just dev` runs this automatically; run it yourself before `just sync` /
# `just relayer`.
ensure-db: (create-db "synchronizer") (create-db "relayer")

# Point the synchronizer + archiver at the current chain head on a *fresh* start.
# Both require INIT_START_SLOT but use it only when their store is empty (they
# resume from on-disk progress otherwise), so we rewrite a service's .env to the
# current beacon head when its db is absent, or when INIT_START_SLOT is unset (the
# var is required, so a resumed service still needs *some* value). Runs as part of
# `just dev`; a no-op once each db exists and the var is set.
ensure-start-slot:
    #!/usr/bin/env bash
    set -uo pipefail

    read_env() {  # <file> <KEY> -> value (uncommented only, quotes/space stripped)
        local file="$1" key="$2" line
        [ -f "$file" ] || return 0
        line="$(grep -E "^[[:space:]]*${key}=" "$file" | tail -n1 || true)"
        [ -n "$line" ] || return 0
        printf '%s' "${line#*=}" | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//' -e 's/^"//' -e 's/"$//' -e "s/^'//" -e "s/'\$//"
    }

    set_env() {  # <file> <KEY> <VALUE> -> upsert, dropping any prior commented/plain line
        local file="$1" key="$2" val="$3" tmp
        tmp="$(mktemp)"
        grep -vE "^[[:space:]]*#?[[:space:]]*${key}=" "$file" > "$tmp" 2>/dev/null || true
        echo "${key}=${val}" >> "$tmp"
        mv "$tmp" "$file"
    }

    head_slot() {  # <beacon_url> -> head slot number
        local beacon="${1%/}" json
        json="$(curl -fsSL "${beacon}/eth/v1/beacon/headers/head")" || return 1
        printf '%s' "$json" | jq -r '.data.header.message.slot'
    }

    ensure_slot() {  # <label> <env_file> <fresh:true|false>
        local label="$1" env="$2" fresh="$3" cur beacon slot
        if [ ! -f "$env" ]; then echo "$label: $env missing; skipping"; return 0; fi
        cur="$(read_env "$env" INIT_START_SLOT)"
        if [ "$fresh" = "false" ] && [ -n "$cur" ]; then
            echo "$label: resuming; INIT_START_SLOT=$cur unchanged"; return 0
        fi
        beacon="$(read_env "$env" BEACON_URL)"
        if [ -z "$beacon" ]; then echo "$label: no BEACON_URL in $env; leaving INIT_START_SLOT as-is"; return 0; fi
        slot="$(head_slot "$beacon" 2>/dev/null || true)"
        if [[ "$slot" =~ ^[0-9]+$ ]]; then
            set_env "$env" INIT_START_SLOT "$slot"
            echo "$label: INIT_START_SLOT -> $slot (head)"
        else
            echo "$label: could not resolve beacon head; leaving INIT_START_SLOT as-is"
        fi
    }

    sync_db="$(read_env services/synchronizer/.env APP_STATE_DB_PATH)"; sync_db="${sync_db:-data/synchronizer-db}"
    [ -d "$sync_db" ] && sync_fresh=false || sync_fresh=true
    ensure_slot synchronizer services/synchronizer/.env "$sync_fresh"

    blobs="$(read_env services/archiver/.env BLOBS_PATH)"; blobs="${blobs:-/tmp/blobs/}"
    [ -d "${blobs%/}/by_slot" ] && arch_fresh=false || arch_fresh=true
    ensure_slot archiver services/archiver/.env "$arch_fresh"

# Wipe local state (RocksDB + local Postgres DBs + objects + archiver blobs)
reset:
    #!/usr/bin/env bash
    set -uo pipefail
    [ -x ~/.dobj/bin/dobj ] && ~/.dobj/bin/dobj stop || true
    rm -rf data/ ~/.dobj
    command -v claude >/dev/null 2>&1 && claude mcp remove dobj 2>/dev/null && echo "removed: dobj MCP registration" || true
    for db in synchronizer relayer synchronizer_local relayer_local; do
        psql postgres://postgres@localhost:5432/postgres -c "DROP DATABASE IF EXISTS $db;" || true
    done
    blobs="$(sed -n 's/^[[:space:]]*BLOBS_PATH=//p' services/archiver/.env 2>/dev/null | tail -n1 | tr -d '"' | sed 's/[[:space:]]*$//')"
    blobs="${blobs:-/tmp/blobs/}"
    case "$blobs" in
        /|"") echo "refusing to rm blobs path '$blobs'" ;;
        *) rm -rf "$blobs" && echo "removed archiver blobs: $blobs" ;;
    esac

# Run all tests (except ignored)
test:
    cargo test --workspace --release

# Run all ignored test
test-ignored:
    cargo test --workspace --release -- --ignored --nocapture

# Run the slow end-to-end proof test
test-e2e:
    cargo test -p synchronizer test_e2e_real_proof --release -- --ignored --nocapture

# Build all workspace crates
build:
    cargo build --workspace

# Build all plugins into target/pexe/*.pexe
pack-plugins:
    cargo run -p pexe --release -- build examples/*

# Build and install plugins into ~/.dobj/actions/
install-plugins:
    cargo run -p pexe --release -- build --install examples/*

# Run the `pexe` CLI with arbitrary args. Example:
#   just pexe inspect plan --action CraftWood examples/craft-basics
pexe *ARGS:
    cargo run -p pexe --release -- {{ARGS}}

# Run the dobj `cli` CLI with arbitrary args. Example:
#   just cli inspect-action craft-basics::FindLog
cli *ARGS:
    cargo run -p cli --release -- {{ARGS}}
