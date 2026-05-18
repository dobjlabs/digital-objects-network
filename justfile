# zk-craft justfile
# Install just: https://github.com/casey/just

# Run the synchronizer (loads env from synchronizer/.env if present)
sync:
    RUST_LOG=info cargo run -p synchronizer --release

# Run the relayer (loads env from relayer/.env if present)
relayer:
    RUST_LOG=info cargo run -p relayer --release

# Run the gui
gui:
    cd app-gui && RUST_BACKTRACE=1 RUST_LOG=info pnpm tauri dev --release

# Run relayer + synchronizer + gui together via mprocs
# https://github.com/pvolok/mprocs
dev: ensure-plugins
    mprocs --config mprocs.yaml

# Install plugins into ~/.dobj/actions/ if none are present. Runs as part of
# `just dev` so a fresh clone (or a `just reset`-ed dev env) boots cleanly.
ensure-plugins:
    @mkdir -p ~/.dobj/actions
    @if [ -z "$(find ~/.dobj/actions -maxdepth 1 -name '*.pexe' -print -quit)" ]; then \
        echo "No .pexe plugins installed — packaging from plugins/ and installing..."; \
        just install-plugins; \
    fi

# Wipe local state (RocksDB + local Postgres DBs + objects)
reset:
    rm -rf data/ ~/.dobj
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
