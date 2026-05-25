# bitcraft justfile
# Install just: https://github.com/casey/just

# Which plugin under plugins/<EPISODE>/ `just dev` packages + installs to
# ~/.dobj/actions/. Bitcraft no longer ships any per-class command files —
# users author their own commands via `create-command` once a plugin is loaded.
# `commands/` only contains the meta commands (start, help, create-command,
# preview, consult-docs), which are plugin-agnostic and always installed.
#
# Override per-invocation: `just dev EPISODE=craft-basics`
# Or via env: `BITCRAFT_EPISODE=craft-basics just dev`
EPISODE := env_var_or_default("BITCRAFT_EPISODE", "episode-1")

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
dev: ensure-plugins ensure-commands ensure-mcp
    mprocs --config mprocs.yaml

# Like `just dev`, but without spawning the local synchronizer + relayer —
# point dobjd at the hosted public endpoints instead. Faster spin-up when
# you don't need to fork the chain locally and don't want a local Postgres.
# Uses the standard 7717 default (same as `just dev`).
dev-remote: ensure-plugins ensure-commands ensure-remote-settings ensure-mcp
    mprocs --config mprocs.remote.yaml

# Idempotently point ~/.dobj/settings.json at the hosted synchronizer +
# relayer. Preserves any other keys already in the file.
ensure-remote-settings:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p ~/.dobj
    python3 - <<'PY'
    import json
    from pathlib import Path
    p = Path.home() / '.dobj' / 'settings.json'
    try:
        data = json.loads(p.read_text())
        if not isinstance(data, dict):
            data = {}
    except Exception:
        data = {}
    data['synchronizerApiUrl'] = 'http://18.217.144.33:3000'
    data['relayerApiUrl'] = 'http://18.217.144.33:3200'
    p.write_text(json.dumps(data, indent=2) + '\n')
    print(f"~/.dobj/settings.json → hosted sync ({data['synchronizerApiUrl']}) + relayer ({data['relayerApiUrl']})")
    PY

# Install the EPISODE plugin into ~/.dobj/actions/ if missing, AND prune any
# OTHER plugins (e.g. swapping from craft-basics → episode-1 leaves the old
# craft-basics.pexe lying around, which would shadow class/action lookups).
# Runs as part of `just dev` so a fresh clone (or `just reset`-ed dev env)
# boots cleanly with exactly one plugin loaded.
ensure-plugins:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p ~/.dobj/actions
    # Remove stale .pexe files that aren't the active episode.
    for f in ~/.dobj/actions/*.pexe; do
        [ -f "$f" ] || continue
        base=$(basename "$f" .pexe)
        if [ "$base" != "{{EPISODE}}" ]; then
            echo "pruning stale plugin: $base.pexe (active episode: {{EPISODE}})"
            rm -f "$f"
        fi
    done
    # Install the active episode if it's not already there.
    if [ ! -f ~/.dobj/actions/{{EPISODE}}.pexe ]; then
        echo "Installing plugins/{{EPISODE}}..."
        just install-plugins
    fi

# Register the bitcraft MCP with Claude Code at project (default) scope, so it
# only loads in chats started from this directory. Other directories stay
# uncontaminated by the bitcraft dispatch rules. Idempotent: remove + add on
# each run so the URL stays current. Skipped silently if the `claude` CLI is
# missing.
ensure-mcp:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! command -v claude >/dev/null 2>&1; then
        exit 0
    fi
    claude mcp remove bitcraft 2>/dev/null || true
    claude mcp add --transport http bitcraft http://127.0.0.1:7718/mcp \
        && echo "registered: bitcraft MCP (project scope, http://127.0.0.1:7718/mcp)"

# Install bitcraft meta commands into ~/.claude/skills/ if none are present
# yet (fresh clone, or post-`just reset`). Re-run `just install-commands`
# manually after editing a SKILL.md in commands/.
ensure-commands:
    @mkdir -p ~/.claude/skills
    @if [ -z "$(find ~/.claude/skills -maxdepth 1 -type d -name 'bitcraft-*' -print -quit)" ]; then \
        echo "No bitcraft commands installed — installing from commands/..."; \
        just install-commands; \
    fi

# Wipe local state: RocksDB, local Postgres DBs, objects, installed bitcraft
# commands, the bitcraft-preview entry in ~/.claude/launch.json, and the
# SessionStart compact hook in ~/.claude/settings.json.
reset:
    @[ -x ~/.dobj/bin/dobj ] && ~/.dobj/bin/dobj stop || true
    rm -rf data/ ~/.dobj
    rm -rf ~/.claude/skills/bitcraft-*
    @python3 commands/start/ensure_launch.py --remove && echo "removed: bitcraft-preview from ~/.claude/launch.json"
    @python3 commands/start/ensure_hook.py --remove && echo "removed: bitcraft compact hook from ~/.claude/settings.json"
    @command -v claude >/dev/null 2>&1 && claude mcp remove bitcraft 2>/dev/null && echo "removed: bitcraft MCP registration" || true
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

# Build all plugins (every dir under plugins/) into target/pexe/*.pexe. Useful
# for release builds where you want every episode's pexe artifact.
pack-plugins:
    cargo run -p pexe --release -- build plugins/*

# Build and install ONLY the active EPISODE's plugin into ~/.dobj/actions/.
# Use `just pack-plugins` if you want every plugin built (release pipelines).
install-plugins:
    cargo run -p pexe --release -- build --install plugins/{{EPISODE}}

# Run the `pexe` CLI with arbitrary args. Example:
#   just pexe inspect plan --action CraftWood plugins/craft-basics
pexe *ARGS:
    cargo run -p pexe --release -- {{ARGS}}

# Run the dobj `cli` CLI with arbitrary args. Example:
#   just cli inspect-action craft-basics::FindLog
cli *ARGS:
    cargo run -p cli --release -- {{ARGS}}

# Install bitcraft meta commands into ~/.claude/skills/bitcraft-*/. Copies
# each commands/<name>/ directory (SKILL.md + any sibling files like index.html
# or helper scripts) to ~/.claude/skills/bitcraft-<name>/.
#
# Wipes each target directory before copy, so renaming a sibling file in
# commands/<name>/ is reflected. ONLY wipes the names we're about to reinstall
# — user-authored commands (written by `create-command` to ~/.claude/skills/
# bitcraft-<name>/) survive `just install-commands` because they don't appear
# in commands/.
#
# Also registers the compact-re-injection hook in ~/.claude/settings.json
# (idempotent).
install-commands:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p ~/.claude/skills
    install_dir() {
        local dir="$1"
        local name
        name=$(basename "$dir")
        local target="$HOME/.claude/skills/bitcraft-$name"
        rm -rf "$target"
        mkdir -p "$target"
        cp -R "$dir"* "$target/"
        echo "installed: bitcraft-$name"
    }
    for dir in commands/*/; do
        install_dir "$dir"
    done
    if [ -f ~/.claude/skills/bitcraft-start/ensure_hook.py ]; then
        python3 ~/.claude/skills/bitcraft-start/ensure_hook.py && echo "registered: SessionStart compact hook"
    fi
