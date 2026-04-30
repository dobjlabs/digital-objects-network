# zk-craft justfile
# Install just: https://github.com/casey/just
#
# Action validators are now baked into the `craft-actions` crate at compile
# time (committed to the risc0 guest's image_id), so there's no separate
# plugin install step — `just dev` just runs the three services together.

# Run the synchronizer (loads env from synchronizer/.env if present)
sync:
    RUST_LOG=info cargo run -p synchronizer --release

# Run the relayer (loads env from relayer/.env if present)
relayer:
    RUST_LOG=info cargo run -p relayer --release

# Run the gui
gui:
    cd app-gui && RUST_LOG=info pnpm tauri dev --release

# Run relayer + synchronizer + gui together via mprocs
# https://github.com/pvolok/mprocs
dev:
    mprocs --config mprocs.yaml

# Wipe local state (objects, local Postgres DBs)
reset:
    rm -rf ~/.dobj
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

# Print the current GUEST_IMAGE_ID for the compiled craft-guest. Drop this
# value into synchronizer/.env and relayer/.env when you change craft-actions.
print-image-id:
    @cargo run --quiet --release --example print_image_id -p craft-methods
