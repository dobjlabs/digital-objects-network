# syntax=docker/dockerfile:1

# Builds the two headless server binaries into separate, minimal images that
# share one cooked dependency layer. Build either one independently:
#
#   docker build --target synchronizer -t zkcraft/synchronizer .
#   docker build --target relayer      -t zkcraft/relayer .
#
# --target synchronizer compiles only the synchronizer binary (and vice versa);
# the expensive dependency compile is cooked once and reused by both.

# ---- chef: pinned toolchain + cargo-chef, shared by every build stage ----
# Build and run on the same Debian release so the binary's glibc, libssl, and
# other shared libraries match between the builder and the runtime stages.
FROM debian:trixie-slim AS chef
ENV CARGO_HOME=/usr/local/cargo \
    RUSTUP_HOME=/usr/local/rustup \
    PATH=/usr/local/cargo/bin:$PATH \
    CARGO_TERM_COLOR=always \
    DEBIAN_FRONTEND=noninteractive
# clang + libclang + cmake: rocksdb's build.rs runs bindgen and compiles the
# vendored C++ lib. pkg-config + libssl-dev: reqwest's default-tls links
# openssl-sys. build-essential: the C/C++ toolchain both of those need.
RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential clang libclang-dev cmake pkg-config libssl-dev \
      curl ca-certificates git \
    && rm -rf /var/lib/apt/lists/*
# Install the exact channel from rust-toolchain.toml here so it is a cached
# layer and the version is explicit. Keep this ARG in sync with that file.
ARG RUST_TOOLCHAIN=nightly-2026-01-25
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --no-modify-path --default-toolchain "$RUST_TOOLCHAIN" --profile minimal \
    && rustc --version && cargo --version
RUN cargo install cargo-chef --locked
WORKDIR /app

# ---- planner: distill the dependency graph into recipe.json ----
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# ---- cook: compile the shared dependency graph once ----
# pod2 + plonky2 + rocksdb + alloy dominate build time. Cooking them here makes
# a cached layer both services reuse, which only changes when a dependency
# changes. The -p scope also keeps the Tauri desktop crate (app-gui/src-tauri,
# which needs GTK/WebKit) out of the build entirely.
FROM chef AS cook
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json \
      -p synchronizer -p relayer

# ---- per-service builds: each target compiles only its own binary ----
# Each logs the binary's dynamic deps so a dependency bump that pulls in a new
# C library is caught here and not at runtime.
FROM cook AS build-synchronizer
COPY . .
RUN cargo build --release --locked -p synchronizer
RUN echo "--- synchronizer ldd ---" && ldd target/release/synchronizer || true

FROM cook AS build-relayer
COPY . .
RUN cargo build --release --locked -p relayer
RUN echo "--- relayer ldd ---" && ldd target/release/relayer || true

# ---- runtime base: shared minimal runtime, non-root user ----
# Runtime libraries both binaries dynamically link, confirmed via the ldd output
# above: TLS (libssl3 provides libssl + libcrypto), zlib and zstd (rocksdb and
# others), and libgcc. ca-certificates verifies TLS to the Ethereum endpoints
# and Postgres. All of these package names exist on amd64 and arm64. The service
# user gets a fixed uid/gid so the synchronizer volume's ownership survives image
# rebuilds and upgrades.
FROM debian:trixie-slim AS runtime
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates libssl3 libgcc-s1 zlib1g libzstd1 \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 10001 zkcraft \
    && useradd --system --uid 10001 --gid 10001 --create-home --home-dir /home/zkcraft --shell /usr/sbin/nologin zkcraft

# ---- synchronizer image ----
FROM runtime AS synchronizer
# Only the synchronizer links the C++ runtime: rocksdb is statically linked into
# the binary but pulls in libstdc++ (the relayer has no C++ dependency).
RUN apt-get update && apt-get install -y --no-install-recommends \
      libstdc++6 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build-synchronizer /app/target/release/synchronizer /usr/local/bin/synchronizer
RUN mkdir -p /var/lib/zkcraft && chown -R zkcraft:zkcraft /var/lib/zkcraft
# Bind all interfaces: inside the container the orchestrator or reverse proxy
# controls public exposure. RocksDB lives on a path meant to be a mounted volume
# (it is a rebuildable cache, but a volume avoids slow cold re-sync on restart).
ENV HTTP_BIND=0.0.0.0:3000 \
    APP_STATE_DB_PATH=/var/lib/zkcraft/synchronizer-db
VOLUME ["/var/lib/zkcraft"]
USER zkcraft
EXPOSE 3000
ENTRYPOINT ["/usr/local/bin/synchronizer"]

# ---- relayer image ----
FROM runtime AS relayer
COPY --from=build-relayer /app/target/release/relayer /usr/local/bin/relayer
# No local state: all relayer state is in Postgres, so no volume.
ENV HTTP_BIND=0.0.0.0:3200
USER zkcraft
EXPOSE 3200
ENTRYPOINT ["/usr/local/bin/relayer"]
