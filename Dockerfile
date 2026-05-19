# Multi-stage Dockerfile for hosting one bitcraft A2A agent on Render
# (or any container PaaS). Same image runs as any of the four agents —
# pick which one via the `AGENT` env var (lumberjack | stonemason |
# craftsmith | concierge).
#
# Layout inside the runtime stage:
#   /app/dobjd/dobjd           — Rust daemon (binds DOBJD_PORT + MCP port)
#   /app/dobjd/.libs/          — libscip + GCC runtime libs (rpath target)
#   /app/dobjd/actions/craft-basics.pexe
#   /app/agents/               — Python A2A server source (uv-installed
#                                 into the system Python)
#   /usr/local/bin/entrypoint.sh
#
# At boot, entrypoint.sh:
#   1. Writes ~/.dobj/settings.json from $SYNC_URL/$RELAY_URL
#   2. Symlinks the plugin into ~/.dobj/actions/
#   3. Starts dobjd in the background on DOBJD_PORT
#   4. Polls dobjd /healthz until ready
#   5. exec's `python -m $AGENT` on $PORT (Render's external port)

# ---------------------------------------------------------------------------
# Stage 1 — Rust build (dobjd + pexe + the craft-basics plugin)
# ---------------------------------------------------------------------------
# Pin to ubuntu 22.04 / glibc 2.35 to match the release CI (release.yml).
# scip-sys's cmake step links libscip against the system gfortran; an
# older base widens compatibility with what Render's worker provides.
FROM rust:1.83-slim-bookworm AS rust-build

# scip-sys vendors SCIP and builds it via cmake on first compile.
# Without these, you'll see "command not found: cmake" or undefined
# Fortran symbols deep inside the scip-sys build script.
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential \
        cmake \
        gfortran \
        pkg-config \
        ca-certificates \
        git \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src
COPY . .

# Build dobjd (release) and the pexe builder. `--locked` mirrors release CI.
# This is the slow step — ~15-30 min cold, seconds with Render's build cache.
RUN cargo build --release --locked -p dobjd -p pexe

# Build the craft-basics plugin into a .pexe artifact. The pexe binary
# takes a plugin source dir and emits a single .pexe file. Output path
# matches what bootstrap_dobjds.sh expects on the host.
RUN ./target/release/pexe build plugins/craft-basics && \
    ls -la target/pexe/

# ---------------------------------------------------------------------------
# Stage 2 — Python runtime (the A2A server + colocated dobjd)
# ---------------------------------------------------------------------------
FROM python:3.12-slim-bookworm AS runtime

# Runtime shared libs that libscip dynamically links against. The build
# stage's rpath is `$ORIGIN/.libs` (see dobjd/build.rs), so libscip itself
# is bundled in /app/dobjd/.libs/ below — but its GCC-runtime deps
# (libgfortran, libquadmath, libgcc_s) need to be present in the system
# loader path too. apt provides ABI-stable runtimes; copying them from
# the build stage works but couples us to the build container's exact
# package versions. apt is more portable.
#
# curl is for the entrypoint's /healthz poll loop. ca-certificates is
# for outbound HTTPS to Anthropic / GEMINI / OpenAI APIs.
RUN apt-get update && apt-get install -y --no-install-recommends \
        libgfortran5 \
        libquadmath0 \
        libgcc-s1 \
        curl \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# uv is the fastest Python package manager that knows pyproject.toml.
# Copying the binary from the official image is the documented pattern.
COPY --from=ghcr.io/astral-sh/uv:latest /uv /uvx /usr/local/bin/

# ---- dobjd binary + libs ---------------------------------------------------
WORKDIR /app/dobjd

COPY --from=rust-build /src/target/release/dobjd ./dobjd

# scip-sys writes libscip into a hash-suffixed dir under target/ —
# resolve with a glob, then copy into the .libs/ sibling that dobjd's
# baked-in rpath points to (see dobjd/build.rs:37 → "$ORIGIN/.libs").
RUN mkdir -p .libs
COPY --from=rust-build /src/target/release/build .libs-staging
RUN set -eux; \
    find .libs-staging -name 'libscip.so*' -exec cp -L {} .libs/ \; ; \
    find .libs-staging -name 'libgfortran.so*' -exec cp -L {} .libs/ \; 2>/dev/null || true ; \
    rm -rf .libs-staging ; \
    ls -la .libs/

# Sanity-check that dobjd can find its libs at runtime BEFORE the
# container ships. If this fails, the rpath/.libs staging is wrong.
RUN ldd /app/dobjd/dobjd | grep -i scip || \
    (echo "WARN: dobjd does not dynamically link libscip — either statically linked (fine) or the rpath is broken (not fine)" && true)

# Plugin .pexe — entrypoint will symlink this into the per-agent
# ~/.dobj/actions/ at boot.
RUN mkdir -p /app/dobjd/actions
COPY --from=rust-build /src/target/pexe/craft-basics.pexe /app/dobjd/actions/

# ---- Python agents ---------------------------------------------------------
WORKDIR /app/agents

# Install Python deps + register the four agent packages on sys.path.
# `--system` skips creating a venv; the container is the isolation
# boundary. We install the project itself (not just deps) so that
# `python -m lumberjack` etc. work regardless of cwd, and so the four
# agent packages defined in pyproject.toml's hatch wheel target end up
# importable system-wide.
COPY agents/ ./
RUN uv pip install --system --no-cache-dir .

# ---- Entrypoint ------------------------------------------------------------
COPY agents/scripts/docker_entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh

# Render injects PORT at runtime. Default for local docker run.
ENV PORT=9996 \
    DOBJD_PORT=7717 \
    A2A_HOST=0.0.0.0 \
    DOBJD_URL=http://127.0.0.1:7717 \
    PYTHONUNBUFFERED=1 \
    PYTHONDONTWRITEBYTECODE=1 \
    AGENT=lumberjack

EXPOSE 9996
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
