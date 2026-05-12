# bitcraft justfile
# Install just: https://github.com/casey/just

# Run the synchronizer (loads env from synchronizer/.env if present)
sync:
    RUST_LOG=info cargo run -p synchronizer --release

# Run the relayer (loads env from relayer/.env if present)
relayer:
    RUST_LOG=info cargo run -p relayer --release

# Run the desktop app standalone (Tauri spawns its own Vite on :1420).
# Use this when you only want the desktop window. Inside `just dev` we use
# `desktop-shell` instead so a shared Vite serves both desktop and browser.
desktop:
    cd app-gui && RUST_BACKTRACE=1 RUST_LOG=info pnpm tauri dev --release

# Run the Tauri shell pointing at an *already-running* Vite at :1420.
# Skips Tauri's `beforeDevCommand` so it doesn't fight the standalone web
# pane for the port. Pair with `just web`.
desktop-shell:
    cd app-gui && RUST_LOG=info pnpm tauri dev --release -c '{"build":{"beforeDevCommand":""}}'

# Run the Vite dev server alone on :1420. Reachable from any browser tab
# or from the Tauri shell. Talks to dobjd at :7717 over HTTP for everything
# driver-related.
web:
    cd app-gui && pnpm dev

# Run the headless HTTP server that exposes the driver API to every client
# (desktop window, browser tab, MCP, dobj CLI).
dobjd:
    RUST_LOG=info cargo run -p dobjd --release

# Bring up everything: synchronizer, relayer, dobjd, Vite, and the Tauri
# shell — all backed by one dobjd process. Open http://localhost:1420 in a
# browser to use the website client; the desktop window opens automatically.
# https://github.com/pvolok/mprocs
dev: ensure-plugins ensure-commands
    mprocs --config mprocs.yaml

# Install plugins into ~/.dobj/actions/ if none are present. Runs as part of
# `just dev` so a fresh clone (or a `just reset`-ed dev env) boots cleanly.
ensure-plugins:
    @mkdir -p ~/.dobj/actions
    @if [ -z "$(find ~/.dobj/actions -maxdepth 1 -name '*.pexe' -print -quit)" ]; then \
        echo "No .pexe plugins installed — packaging from plugins/ and installing..."; \
        just install-plugins; \
    fi

# Install bitcraft commands into ~/.claude/skills/ if the built-ins are missing.
# Runs as part of `just dev`. Re-run `just install-commands` manually after
# editing a command source file in commands/.
ensure-commands:
    @mkdir -p ~/.claude/skills
    @if [ ! -d ~/.claude/skills/bitcraft-chop-log ]; then \
        echo "No bitcraft commands installed — installing from commands/..."; \
        just install-commands; \
    fi

# Wipe local state (RocksDB + local Postgres DBs + objects + installed bitcraft commands)
reset:
    @[ -x ~/.dobj/bin/dobj ] && ~/.dobj/bin/dobj stop || true
    rm -rf data/ ~/.dobj
    rm -rf ~/.claude/skills/bitcraft-*
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
    cargo run -p pexe --release -- build plugins/*

# Build and install plugins into ~/.dobj/actions/
install-plugins:
    cargo run -p pexe --release -- build --install plugins/*

# Run the `pexe` CLI with arbitrary args. Example:
#   just pexe inspect plan --action CraftWood plugins/craft-basics
pexe *ARGS:
    cargo run -p pexe --release -- {{ARGS}}

# Install bitcraft commands (SKILL.md files) into ~/.claude/skills/bitcraft-*/
# Install bitcraft commands into ~/.claude/skills/bitcraft-*/. Copies SKILL.md
# plus any sibling files (e.g. preview.html, sibling scripts).
install-commands:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p ~/.claude/skills
    for dir in commands/*/; do
        name=$(basename "$dir")
        target=~/.claude/skills/bitcraft-"$name"
        mkdir -p "$target"
        cp -R "$dir"* "$target/"
        echo "installed: bitcraft-$name"
    done
