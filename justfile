# zk-craft justfile
# Install just: https://github.com/casey/just

# Run the synchronizer (loads env from synchronizer/.env if present)
sync:
    RUST_LOG=info cargo run -p synchronizer --release

# Run all tests (except ignored)
test:
    cargo test --workspace

# Wipe local synchronizer state (RocksDB + local Postgres DB)
reset-db:
    rm -rf data/
    psql postgres://postgres@localhost:5432/postgres -c 'DROP DATABASE IF EXISTS synchronizer;'

# Run the slow end-to-end proof test 
test-e2e:
    cargo test -p synchronizer test_e2e_real_proof -- --ignored --nocapture

# Build all workspace crates
build:
    cargo build --workspace
