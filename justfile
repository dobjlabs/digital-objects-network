# Digital Objects Network justfile
# Install just: https://github.com/casey/just

# Run the synchronizer (loads env from services/synchronizer/.env if present)
sync:
    RUST_LOG=info cargo run -p synchronizer --release

# Run the relayer (loads env from services/relayer/.env if present)
relayer:
    RUST_LOG=info cargo run -p relayer --release

# Run the archiver (loads env from archiver/.env if present)
archiver:
    RUST_LOG=info cargo run -p archiver --release

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
    cd interfaces/gui && pnpm dev

# Run the headless HTTP server that exposes the driver API to every client
# (desktop window, browser tab, MCP, dobj CLI).
dobjd:
    RUST_LOG=info cargo run -p dobjd --release

# Bring up everything: synchronizer, relayer, dobjd, Vite, and the Tauri
# shell — all backed by one dobjd process. Open http://localhost:1420 in a
# browser to use the website client; the desktop window opens automatically.
# https://github.com/pvolok/mprocs
dev: ensure-db ensure-plugins ensure-mcp
    mprocs --config mprocs.yaml

# Like `just dev`, but without spawning the local synchronizer + relayer —
# point dobjd at the hosted public endpoints instead. Faster spin-up when
# you don't need to fork the chain locally and don't want a local Postgres.
# Uses the standard 7717 default (same as `just dev`).
dev-remote: ensure-remote-settings ensure-plugins ensure-mcp
    mprocs --config mprocs.remote.yaml

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
    @printf '{"synchronizerApiUrl":"https://sync.don.pateldhvani.com","relayerApiUrl":"https://relay.don.pateldhvani.com"}\n' > ~/.dobj/settings.json
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

# Ensure the local Postgres databases the synchronizer + relayer expect exist.
# `just dev` runs this automatically; run it yourself before `just sync` /
# `just relayer`. Idempotent: skips a database that already exists.
ensure-db:
    @psql postgres://postgres@localhost:5432/postgres -tAc "SELECT 1 FROM pg_database WHERE datname='synchronizer'" | grep -q 1 || psql postgres://postgres@localhost:5432/postgres -c 'CREATE DATABASE synchronizer'
    @psql postgres://postgres@localhost:5432/postgres -tAc "SELECT 1 FROM pg_database WHERE datname='relayer'" | grep -q 1 || psql postgres://postgres@localhost:5432/postgres -c 'CREATE DATABASE relayer'

# Wipe local state (RocksDB + local Postgres DBs + objects)
reset:
    @[ -x ~/.dobj/bin/dobj ] && ~/.dobj/bin/dobj stop || true
    rm -rf data/ ~/.dobj
    @command -v claude >/dev/null 2>&1 && claude mcp remove dobj 2>/dev/null && echo "removed: dobj MCP registration" || true
    psql postgres://postgres@localhost:5432/postgres -c 'DROP DATABASE IF EXISTS synchronizer;'
    psql postgres://postgres@localhost:5432/postgres -c 'DROP DATABASE IF EXISTS relayer;'

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
