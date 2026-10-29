# Digital Objects Network justfile
# Install just: https://github.com/casey/just

# Driver state root the ensure-* recipes below act on. They take it as a
# parameter rather than reading DOBJ_HOME themselves: a dependency runs in its
# own process before the depending recipe's body, so it cannot see an export
# the body performs, and `just dev-local` would stock the default root instead
# of its devnet one.
DOBJ_ROOT := env_var_or_default("DOBJ_HOME", home_directory() / ".dobj")

# Devnet counterpart, passed by `just dev-local`. Absolute so every client
# and service resolves the same root.
LOCAL_DOBJ_ROOT := justfile_directory() / "data/dobj-local"
LOCAL_BITCRAFT_ROOT := justfile_directory() / ".dev/bitcraft-local"

# Use --bin for dev commands so dependency features are unified across the
# workspace. Separate -p selections rebuild shared dependencies with different
# feature sets as each service starts.

# Run the synchronizer (loads env from services/synchronizer/.env if present)
sync:
    RUST_LOG=info cargo run --bin synchronizer --release

# Run the relayer (loads env from services/relayer/.env if present)
relayer:
    RUST_LOG=info cargo run --bin relayer --release

# Run the archiver (loads env from services/archiver/.env if present)
archiver:
    RUST_LOG=info cargo run --bin archiver --release

# The 2s block time is load-bearing, and the state file must stay in step with
# the archiver + synchronizer stores; see devtools/beacon-shim/README.md.
# mprocs.local.yaml repeats these flags rather than calling this recipe, so that
# it supervises anvil directly and can signal it on quit. Keep the two in step.
# Run the local anvil devnet standalone.
anvil:
    @mkdir -p data
    anvil --block-time 2 --port 8545 --state data/anvil-state.json --state-interval 5

# Run the Beacon REST shim that projects anvil blocks onto beacon slots for the
# archiver and synchronizer. Waits for anvil, so start order does not matter.
beacon-shim:
    RUST_LOG=info cargo run --bin beacon-shim --release

# Run the browser UI development server on :1420. HTTP/SSE requests are
# proxied to dobjd, so forwarding just the UI port also works.
web:
    cd interfaces/gui && pnpm install && pnpm dev

# Build the default UI and embed it into the release daemon binary.
build-ui:
    cd interfaces/gui && pnpm install --frozen-lockfile && pnpm build

build-dobjd: build-ui
    cargo build --release -p dobjd --features bundled-ui

build-services-docker:
    #!/usr/bin/env bash
    set -euo pipefail

    t=$(git tag --points-at HEAD | head -1)
    h=$(git rev-parse HEAD)
    tag="${t:+$t-}$h"
    echo "release_tag=${tag}"
    docker build --build-arg DOBJ_RELEASE_TAG=${tag} --target synchronizer -t ghcr.io/dobjlabs/digital-objects-network/synchronizer:${tag} .
    docker build --build-arg DOBJ_RELEASE_TAG=${tag} --target relayer      -t ghcr.io/dobjlabs/digital-objects-network/relayer:${tag} .
    docker build --build-arg DOBJ_RELEASE_TAG=${tag} --target archiver     -t ghcr.io/dobjlabs/digital-objects-network/archiver:${tag} .
    docker tag ghcr.io/dobjlabs/digital-objects-network/synchronizer:${tag} \
            ghcr.io/dobjlabs/digital-objects-network/synchronizer:dev
    docker tag ghcr.io/dobjlabs/digital-objects-network/relayer:${tag} \
            ghcr.io/dobjlabs/digital-objects-network/relayer:dev
    docker tag ghcr.io/dobjlabs/digital-objects-network/archiver:${tag} \
            ghcr.io/dobjlabs/digital-objects-network/archiver:dev

bitcraft-shell:
    cargo run --manifest-path apps/bitcraft/src-tauri/Cargo.toml --locked -- --no-runtime

bitcraft-prepare:
    python3 apps/bitcraft/scripts/prepare.py

bitcraft-prepare-dev:
    python3 apps/bitcraft/scripts/prepare.py --dev

# Regenerate bitcraft's presentation pack (icons, summaries, descriptions) and
# copy it into the Bitcraft UI.
bitcraft-presentation:
    python3 examples/bitcraft/presentation/generate.py
    python3 apps/bitcraft/scripts/sync_presentation.py

bitcraft-dev: bitcraft-prepare-dev
    cd apps/bitcraft && npx --yes @tauri-apps/cli@2.12.2 dev --config src-tauri/tauri.bundle.conf.json

bitcraft-dev-local: bitcraft-prepare-dev
    python3 apps/bitcraft/scripts/init_local.py "{{LOCAL_BITCRAFT_ROOT}}"
    cd apps/bitcraft && npx --yes @tauri-apps/cli@2.12.2 dev --config src-tauri/tauri.bundle.conf.json -- -- --home "{{LOCAL_BITCRAFT_ROOT}}"

bitcraft-bundle: bitcraft-prepare
    cd apps/bitcraft && npx --yes @tauri-apps/cli@2.12.2 build --config src-tauri/tauri.bundle.conf.json

# Run the Bitcraft leaderboard bot (loads env from apps/bitcraft/discord-bot/.env if present)
bitcraft-discord-bot:
    cargo run --bin bitcraft-discord-bot --release

# Run the documentation site (Vocs dev server) with hot reload
docs:
    cd docs && pnpm install && pnpm dev

# Run the headless HTTP server that exposes the driver API to every client
# (browser UI, MCP, dobj CLI).
dobjd:
    RUST_LOG=info cargo run --bin dobjd --release

# Bring up synchronizer, relayer, dobjd, and the Vite browser UI.
# Open http://localhost:1420 once the web server is ready.
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
dev-local: ensure-anvil ensure-db-local (ensure-local-settings LOCAL_DOBJ_ROOT) (ensure-plugins LOCAL_DOBJ_ROOT) ensure-mcp (ensure-mcp-enabled LOCAL_DOBJ_ROOT)
    #!/usr/bin/env bash
    set -euo pipefail
    # mprocs.local.yaml execs anvil directly, so nothing else creates its state dir.
    mkdir -p data
    export DOBJ_HOME="{{LOCAL_DOBJ_ROOT}}"
    export RPC_URL=http://127.0.0.1:8545
    export BEACON_URL=http://127.0.0.1:8555
    export ARCHIVER_URL=http://127.0.0.1:3001
    export TO_ADDRESS=0x4343434343434343434343434343434343434343
    export FILTER_ADDRESS="$TO_ADDRESS"
    # anvil's first genesis-funded dev account. Published test key, local only.
    export PRIVATE_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80
    # The synchronizer derives its genesis state at this slot, so it needs an execution block:
    # anvil's block 1, which the synchronizer waits for on a fresh devnet.
    export INIT_START_SLOT=1
    export BLOBS_PATH=data/blobs-local/
    export CONTAINER_DB_PATH=data/synchronizer-db-local
    export STATE_LOG_DB_URL=postgres://postgres@localhost:5432/synchronizer_local
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
ensure-local-settings ROOT=DOBJ_ROOT:
    #!/usr/bin/env bash
    set -euo pipefail
    root="{{ROOT}}"
    f="$root/settings.json"
    mkdir -p "$root"
    cur="{}"; [ -f "$f" ] && cur="$(jq '.' "$f" 2>/dev/null || echo '{}')"
    printf '%s' "$cur" | jq '. + {synchronizerApiUrl:"http://127.0.0.1:3000", relayerApiUrl:"http://127.0.0.1:3200"}' > "$f.tmp"
    mv "$f.tmp" "$f"
    echo "$f -> local sync + relayer"

# Block (up to ~5 min) until an HTTP endpoint responds, then return. mprocs
# uses this to launch synchronizer -> relayer -> dobjd -> web in
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

# Idempotently point the driver's settings.json at the hosted synchronizer + relayer
ensure-remote-settings:
    #!/usr/bin/env bash
    set -euo pipefail
    root="${DOBJ_HOME:-$HOME/.dobj}"
    mkdir -p "$root"
    printf '{"synchronizerApiUrl":"https://terminus.digiobjects.net:4000","relayerApiUrl":"https://terminus.digiobjects.net:4200"}\n' > "$root/settings.json"
    echo "$root/settings.json -> hosted sync + relayer"

# Install plugins into the driver's actions/ dir if none are present. Runs as
# part of `just dev` so a fresh clone (or a `just reset`-ed dev env) boots
# cleanly. `just dev-local` passes its devnet root, which is stocked separately
# from the default one.
ensure-plugins ROOT=DOBJ_ROOT:
    #!/usr/bin/env bash
    set -euo pipefail
    actions="{{ROOT}}/actions"
    mkdir -p "$actions"
    if [ -z "$(find "$actions" -maxdepth 1 -name '*.pexe' -print -quit)" ]; then
        echo "No .pexe plugins in $actions - packaging from examples/ and installing..."
        just install-plugins '{{ROOT}}'
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
ensure-mcp-enabled ROOT=DOBJ_ROOT:
    #!/usr/bin/env bash
    set -euo pipefail
    root="{{ROOT}}"
    f="$root/settings.json"
    mkdir -p "$root"
    cur="{}"; [ -f "$f" ] && cur="$(jq '.' "$f" 2>/dev/null || echo '{}')"
    printf '%s' "$cur" | jq '{synchronizerApiUrl:"http://127.0.0.1:3000", relayerApiUrl:"http://127.0.0.1:3200"} + . + {mcpEnabled:true}' > "$f.tmp"
    mv "$f.tmp" "$f"
    echo "$f -> mcpEnabled=true"

# Create one Postgres database if it is absent. Idempotent.
create-db NAME:
    @psql postgres://postgres@localhost:5432/postgres -tAc "SELECT 1 FROM pg_database WHERE datname='{{NAME}}'" | grep -q 1 || psql postgres://postgres@localhost:5432/postgres -c 'CREATE DATABASE {{NAME}}'

# Ensure the local Postgres databases the synchronizer + relayer expect exist.
# `just dev` runs this automatically; run it yourself before `just sync` /
# `just relayer`.
ensure-db: (create-db "synchronizer") (create-db "relayer")

# Set INIT_START_SLOT in the synchronizer + archiver .env to a recent genesis slot
# (utils/genesis-slot.sh) when the service's store is absent or the var is unset.
# Both use it only on an empty store and resume from stored progress otherwise.
# Runs as part of `just dev`; a no-op once each store exists and the var is set.
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

    ensure_slot() {  # <label> <env_file> <fresh:true|false>
        local label="$1" env="$2" fresh="$3" cur beacon slot
        if [ ! -f "$env" ]; then echo "$label: $env missing; skipping"; return 0; fi
        cur="$(read_env "$env" INIT_START_SLOT)"
        if [ "$fresh" = "false" ] && [ -n "$cur" ]; then
            echo "$label: resuming; INIT_START_SLOT=$cur unchanged"; return 0
        fi
        beacon="$(read_env "$env" BEACON_URL)"
        if [ -z "$beacon" ]; then echo "$label: no BEACON_URL in $env; leaving INIT_START_SLOT as-is"; return 0; fi
        slot="$(utils/genesis-slot.sh "$beacon" 2>/dev/null || true)"
        if [[ "$slot" =~ ^[0-9]+$ ]]; then
            set_env "$env" INIT_START_SLOT "$slot"
            echo "$label: INIT_START_SLOT -> $slot (finalized, with a canonical payload)"
        else
            echo "$label: could not find a finalized slot with a canonical payload; leaving INIT_START_SLOT as-is"
        fi
    }

    container_db="$(read_env services/synchronizer/.env CONTAINER_DB_PATH)"; container_db="${container_db:-data/synchronizer-db}"
    [ -d "$container_db" ] && sync_fresh=false || sync_fresh=true
    ensure_slot synchronizer services/synchronizer/.env "$sync_fresh"

    blobs="$(read_env services/archiver/.env BLOBS_PATH)"; blobs="${blobs:-/tmp/blobs/}"
    [ -d "${blobs%/}/by_slot" ] && arch_fresh=false || arch_fresh=true
    ensure_slot archiver services/archiver/.env "$arch_fresh"

# Wipe local state, including Bitcraft's devnet home. Quit Bitcraft and stop the dev stack first.
reset:
    #!/usr/bin/env bash
    set -uo pipefail
    # The installed CLI always lives under ~/.dobj/bin, even when DOBJ_HOME
    # points the driver's data elsewhere.
    [ -x ~/.dobj/bin/dobj ] && ~/.dobj/bin/dobj stop || true
    rm -rf data/ ~/.dobj "{{LOCAL_BITCRAFT_ROOT}}"
    root="${DOBJ_HOME:-}"
    case "$root" in
        ""|/) ;;
        *) rm -rf "$root" && echo "removed dobj home: $root" ;;
    esac
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

# Install pinned Quint tools and check the protocol models.
models:
    pnpm --dir devtools/models install --frozen-lockfile --ignore-scripts
    pnpm --dir devtools/models check

# Run all tests (except ignored)
test:
    cargo test --workspace --release

# Run all ignored test
test-ignored:
    cargo test --workspace --release -- --ignored --nocapture

# Run the slow end-to-end proof test
test-e2e:
    cargo test -p synchronizer test_e2e_real_proof --release -- --ignored --nocapture

# Prove N FindLog txs, then time how long the relayer and synchronizer take to
# land them all. Reads RELAYER_URL and SYNCHRONIZER_URL from devtools/tests/.env.
throughput-test N:
    cargo run -p tests --bin throughput --release -- {{N}}

# Run ACTION (FindLog or BurnLogs) end to end through the relayer and the
# synchronizer. Reads RELAYER_URL and SYNCHRONIZER_URL from devtools/tests/.env.
client-e2e-test ACTION:
    cargo run -p tests --bin client-e2e --release -- {{ACTION}}

# Build all workspace crates
build:
    cargo build --workspace

# Build all plugins into target/pexe/*.pexe
pack-plugins:
    cargo run --bin pexe --release -- build examples/*

# Build and install plugins into the driver's actions/ dir (default ~/.dobj/actions/)
install-plugins ROOT=DOBJ_ROOT:
    cargo run --bin pexe --release -- build --install --install-dir '{{ROOT}}/actions' examples/*

# Run the `pexe` CLI with arbitrary args. Example:
#   just pexe inspect plan --action CraftWood examples/craft-basics
pexe *ARGS:
    cargo run --bin pexe --release -- {{ARGS}}

# Run the dobj `cli` CLI with arbitrary args. Example:
#   just cli inspect-action craft-basics::FindLog
cli *ARGS:
    cargo run -p cli --release -- {{ARGS}}
