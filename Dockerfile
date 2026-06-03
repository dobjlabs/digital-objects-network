# syntax=docker/dockerfile:1

# Builds the two headless server binaries into separate, minimal images.
# One shared builder compiles both (they share almost the entire dependency
# graph), then two runtime targets each carry a single binary:
#
#   docker build --target synchronizer -t zkcraft/synchronizer .
#   docker build --target relayer      -t zkcraft/relayer .
#
# The second build reuses the first's cached builder layers.

# ---- chef: pinned toolchain + cargo-chef, shared by planner and builder ----
# Debian trixie matches the validated deploy target (deploy/ec2 is Debian 13),
# so glibc and libssl versions line up between this builder and the runtime
# stages below.
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
# layer and the version is explicit, rather than auto-installed on first cargo
# run. Keep this ARG in sync with rust-toolchain.toml.
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

# ---- builder: cook dependencies (cached), then build both binaries ----
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
# Cook only the two server crates' dependency graphs. The -p scope keeps the
# Tauri crate (app-gui/src-tauri, which needs GTK/WebKit system libs) out of
# the build entirely. This layer is cached until a dependency actually changes.
RUN cargo chef cook --release --recipe-path recipe.json \
      -p synchronizer -p relayer
COPY . .
RUN cargo build --release --locked -p synchronizer -p relayer
# Surface each binary's dynamic deps in the build log, so a future dependency
# bump that pulls in a new C library is caught here and not at runtime.
RUN echo "--- synchronizer ldd ---" && ldd target/release/synchronizer || true \
 && echo "--- relayer ldd ---"      && ldd target/release/relayer      || true

# ---- runtime base: shared minimal runtime, non-root user ----
FROM debian:trixie-slim AS runtime
ENV DEBIAN_FRONTEND=noninteractive
# Runtime libraries both binaries dynamically link, confirmed via the builder's
# ldd output: TLS (libssl3 provides libssl + libcrypto), zlib and zstd (rocksdb
# and others), and libgcc. ca-certificates verifies TLS to the Ethereum
# endpoints and Postgres. All of these package names exist on amd64 and arm64.
RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates libssl3 libgcc-s1 zlib1g libzstd1 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home --home-dir /home/zkcraft --shell /usr/sbin/nologin zkcraft

# ---- synchronizer image ----
FROM runtime AS synchronizer
# Only the synchronizer links the C++ runtime: rocksdb is statically linked into
# the binary but pulls in libstdc++ (confirmed via the builder's ldd output; the
# relayer has no C++ dependency). libgcc, zlib, and zstd come from the base.
RUN apt-get update && apt-get install -y --no-install-recommends \
      libstdc++6 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/synchronizer /usr/local/bin/synchronizer
RUN mkdir -p /var/lib/zkcraft && chown -R zkcraft:zkcraft /var/lib/zkcraft
# Bind all interfaces: inside the container network namespace the orchestrator
# or reverse proxy controls public exposure. RocksDB lives on a path meant to
# be a mounted volume (it is a rebuildable cache, but a volume avoids slow cold
# re-sync on restart).
ENV HTTP_BIND=0.0.0.0:3000 \
    APP_STATE_DB_PATH=/var/lib/zkcraft/synchronizer-db
VOLUME ["/var/lib/zkcraft"]
USER zkcraft
EXPOSE 3000
ENTRYPOINT ["/usr/local/bin/synchronizer"]

# ---- relayer image ----
FROM runtime AS relayer
COPY --from=builder /app/target/release/relayer /usr/local/bin/relayer
# No local state: all relayer state is in Postgres, so no volume.
ENV HTTP_BIND=0.0.0.0:3200
USER zkcraft
EXPOSE 3200
ENTRYPOINT ["/usr/local/bin/relayer"]
