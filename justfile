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

# Run all tests (except ignored)
test:
    cargo test --workspace

# Run the slow end-to-end proof test
test-e2e:
    cargo test -p synchronizer test_e2e_real_proof -- --ignored --nocapture

# Build all workspace crates
build:
    cargo build --workspace
