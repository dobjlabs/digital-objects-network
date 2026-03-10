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
    cd app-gui && pnpm tauri dev --release

# Run relayer + synchronizer + gui together via mprocs
dev:
    @if ! command -v mprocs >/dev/null 2>&1; then \
        echo "mprocs is not installed. Install with: brew install mprocs"; \
        exit 1; \
    fi
    mprocs --config mprocs.yaml

# Initialize local env files from examples (non-destructive)
env-init:
    @if [ ! -f synchronizer/.env ]; then \
        cp synchronizer/.env.example synchronizer/.env; \
        echo "created synchronizer/.env"; \
    else \
        echo "kept synchronizer/.env"; \
    fi
    @if [ ! -f relayer/.env ]; then \
        cp relayer/.env.example relayer/.env; \
        echo "created relayer/.env"; \
    else \
        echo "kept relayer/.env"; \
    fi
    @if [ ! -f app-gui/.env ]; then \
        cp app-gui/.env.example app-gui/.env; \
        echo "created app-gui/.env"; \
    else \
        echo "kept app-gui/.env"; \
    fi

# Run all tests (except ignored)
test:
    cargo test --workspace

# Wipe local state (RocksDB + local Postgres DB + objects)
reset:
    rm -rf data/ ~/.objects
    psql postgres://postgres@localhost:5432/postgres -c 'DROP DATABASE IF EXISTS synchronizer;'

# Run the slow end-to-end proof test
test-e2e:
    cargo test -p synchronizer test_e2e_real_proof -- --ignored --nocapture

# Run ignored Postgres-backed sync_db tests
test-sync-db:
    cargo test -p synchronizer sync_db::tests:: -- --ignored --nocapture

# Build all workspace crates
build:
    cargo build --workspace
