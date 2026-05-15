#!/usr/bin/env bash
# Spin up four isolated dobjd instances on this machine.
#
# Each instance gets its own fake $HOME so its ~/.dobj/ is private:
#   .runtime/lumberjack/.dobj/   port 7717  (MCP 7718)
#   .runtime/stonemason/.dobj/   port 7727  (MCP 7728)
#   .runtime/craftsmith/.dobj/   port 7737  (MCP 7738)
#   .runtime/concierge/.dobj/    port 7747  (MCP 7748)
#
# Each gets:
#   - a copy of craft-basics.pexe from the host's ~/.dobj/actions/
#   - a settings.json pointing at the hosted synchronizer + relayer
#
# Prereqs:
#   - `cargo build -p dobjd --release` (or `just dobjd` to start once)
#   - `just install-plugins` (so craft-basics.pexe exists in ~/.dobj/actions/)
#
# Pass --hosted (default) to use the public synchronizer + relayer,
# or --local to point at http://127.0.0.1:3000 / :3200 (which you'd run
# yourself via `just sync` + `just relayer`).

set -euo pipefail
cd "$(dirname "$0")/.."

MODE="${1:---hosted}"

REPO_ROOT="$(cd ../.. 2>/dev/null && pwd)"
if [ ! -d "$REPO_ROOT/target" ]; then
  REPO_ROOT="$(cd .. && pwd)"
fi

DOBJD_BIN="$REPO_ROOT/target/release/dobjd"
if [ ! -x "$DOBJD_BIN" ]; then
  DOBJD_BIN="$REPO_ROOT/target/debug/dobjd"
fi
if [ ! -x "$DOBJD_BIN" ]; then
  echo "no dobjd binary at $REPO_ROOT/target/(release|debug)/dobjd" >&2
  echo "build first: cargo build -p dobjd --release" >&2
  exit 1
fi

# dobjd's rpath points at @loader_path/.libs and /opt/homebrew/opt/scipopt/lib.
# Neither exists locally, but the build produced libscip in target/Frameworks/
# (or an .app bundle for installed users). Locate one and put it on
# DYLD_LIBRARY_PATH so dyld can find it at launch.
SCIP_LIB_DIR=""
for candidate in \
  "$REPO_ROOT/target/Frameworks" \
  "/opt/homebrew/opt/scipopt/lib" \
  "/usr/local/opt/scipopt/lib" \
  "/Applications/zk-craft.app/Contents/Frameworks"; do
  if [ -f "$candidate/libscip.9.2.dylib" ]; then
    SCIP_LIB_DIR="$candidate"
    break
  fi
done
# Last resort: search anywhere under target/ (covers debug builds and odd hash dirs)
if [ -z "$SCIP_LIB_DIR" ]; then
  found=$(find "$REPO_ROOT/target" -name 'libscip.9.2.dylib' -type f 2>/dev/null | head -1)
  if [ -n "$found" ]; then
    SCIP_LIB_DIR="$(dirname "$found")"
  fi
fi
if [ -z "$SCIP_LIB_DIR" ]; then
  echo "could not locate libscip.9.2.dylib — try: cargo build -p dobjd --release" >&2
  exit 1
fi
echo "libscip found at: $SCIP_LIB_DIR"

SOURCE_PEXE="$HOME/.dobj/actions/craft-basics.pexe"
if [ ! -f "$SOURCE_PEXE" ]; then
  echo "no plugin at $SOURCE_PEXE" >&2
  echo "install first: just install-plugins" >&2
  exit 1
fi

case "$MODE" in
  --hosted)
    SYNC_URL="http://18.217.144.33:3000"
    RELAY_URL="http://18.217.144.33:3200"
    ;;
  --local)
    SYNC_URL="http://127.0.0.1:3000"
    RELAY_URL="http://127.0.0.1:3200"
    ;;
  *)
    echo "usage: $0 [--hosted|--local]" >&2
    exit 1
    ;;
esac

RUNTIME_DIR="$(pwd)/.runtime"
mkdir -p "$RUNTIME_DIR"

# Each agent's name → http port
declare -a AGENTS=(lumberjack stonemason craftsmith concierge)
declare -A PORTS=(
  [lumberjack]=7717
  [stonemason]=7727
  [craftsmith]=7737
  [concierge]=7747
)

prepare_one() {
  local name="$1"
  local home_dir="$RUNTIME_DIR/$name"
  local dobj_dir="$home_dir/.dobj"
  mkdir -p "$dobj_dir/actions"
  # `install` is idempotent across macOS BSD and Linux (unlike `cp -n` which
  # returns non-zero on macOS when the destination exists, tripping `set -e`).
  install -m 644 "$SOURCE_PEXE" "$dobj_dir/actions/craft-basics.pexe"
  cat > "$dobj_dir/settings.json" <<EOF
{
  "synchronizerApiUrl": "$SYNC_URL",
  "relayerApiUrl": "$RELAY_URL"
}
EOF
}

launch_one() {
  local name="$1"
  local port="${PORTS[$name]}"
  local home_dir="$RUNTIME_DIR/$name"
  local log_file="$home_dir/dobjd.log"
  echo "[$name] dobjd on :$port  (home=$home_dir)"
  : > "$log_file"
  DYLD_LIBRARY_PATH="$SCIP_LIB_DIR" \
  DYLD_FALLBACK_LIBRARY_PATH="$SCIP_LIB_DIR" \
  HOME="$home_dir" DOBJD_PORT="$port" "$DOBJD_BIN" >>"$log_file" 2>&1 &
  echo $! > "$home_dir/dobjd.pid"
}

# Returns 0 if dobjd's HTTP port is responding to /healthz, 1 otherwise.
healthcheck_one() {
  local port="$1"
  curl -fsS --max-time 1 "http://127.0.0.1:$port/healthz" >/dev/null 2>&1
}

pids=()
cleanup() {
  echo
  echo "shutting down dobjds…"
  for name in "${AGENTS[@]}"; do
    local pidfile="$RUNTIME_DIR/$name/dobjd.pid"
    if [ -f "$pidfile" ]; then
      kill "$(cat "$pidfile")" 2>/dev/null || true
      rm -f "$pidfile"
    fi
  done
}
trap cleanup EXIT INT TERM

echo "preparing dobjd home dirs under $RUNTIME_DIR"
for name in "${AGENTS[@]}"; do
  prepare_one "$name"
done

echo
echo "launching dobjds against $SYNC_URL (synchronizer) + $RELAY_URL (relayer)…"
for name in "${AGENTS[@]}"; do
  launch_one "$name"
done

# Wait up to 30s for every dobjd to answer /healthz; report any that crashed.
echo
echo "waiting for dobjds to come up…"
all_up=1
for name in "${AGENTS[@]}"; do
  port="${PORTS[$name]}"
  for i in $(seq 1 30); do
    if healthcheck_one "$port"; then
      echo "  [$name] :$port  ready"
      break
    fi
    # Bail if the process died (e.g. dylib not found)
    pid="$(cat "$RUNTIME_DIR/$name/dobjd.pid" 2>/dev/null || echo)"
    if [ -n "$pid" ] && ! kill -0 "$pid" 2>/dev/null; then
      echo "  [$name] :$port  CRASHED — see $RUNTIME_DIR/$name/dobjd.log"
      all_up=0
      break
    fi
    sleep 1
  done
  if [ "$i" -eq 30 ] && ! healthcheck_one "$port"; then
    echo "  [$name] :$port  TIMEOUT — see $RUNTIME_DIR/$name/dobjd.log"
    all_up=0
  fi
done

if [ "$all_up" -ne 1 ]; then
  echo
  echo "one or more dobjds failed to start.  recent log tails:"
  for name in "${AGENTS[@]}"; do
    echo "--- $name ---"
    tail -n 10 "$RUNTIME_DIR/$name/dobjd.log" 2>/dev/null || echo "(no log)"
  done
  exit 1
fi

echo
echo "all four dobjds healthy."
echo
echo "leave this terminal up.  in another:  bash scripts/run_all.sh"
echo "ctrl-C here stops all four."
wait
