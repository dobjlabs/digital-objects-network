#!/bin/sh
# Digital Objects driver installer (macOS / Linux).
#
#   curl -fsSL https://raw.githubusercontent.com/dobjlabs/digital-objects-network/main/install.sh | sh
#
# Installs `dobj` (CLI), `dobjd` (daemon), and `dobj-mcp-proxy` from the
# latest published release into ~/.dobj/bin. Pin a version with:
#
#   DOBJ_VERSION=v0.1.0 curl -fsSL ... | sh
#
# No plugins are installed: the daemon starts with an empty action catalog.
# See INSTALL.md for adding plugins and connecting agents. Safe to re-run;
# re-running installs the latest release over the previous one.

set -eu

REPO="dobjlabs/digital-objects-network"
BIN_DIR="$HOME/.dobj/bin"

say() { printf '%s\n' "$*"; }
err() { printf 'install.sh: %s\n' "$*" >&2; exit 1; }

# --- platform ---------------------------------------------------------------

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64)  TARGET=aarch64-apple-darwin ;;
  Darwin-x86_64) TARGET=x86_64-apple-darwin ;;
  Linux-x86_64)  TARGET=x86_64-unknown-linux-gnu ;;
  MINGW*|MSYS*|CYGWIN*)
    err "this is the macOS/Linux installer; on Windows run:
  irm https://raw.githubusercontent.com/$REPO/main/install.ps1 | iex" ;;
  *) err "unsupported platform: $(uname -sm)" ;;
esac

command -v curl >/dev/null 2>&1 || err "curl is required"
command -v tar  >/dev/null 2>&1 || err "tar is required"

# --- release URL ------------------------------------------------------------

if [ -n "${DOBJ_VERSION:-}" ]; then
  BASE="https://github.com/$REPO/releases/download/$DOBJ_VERSION"
  say "installing pinned version $DOBJ_VERSION ($TARGET)"
else
  BASE="https://github.com/$REPO/releases/latest/download"
  say "installing latest release ($TARGET)"
fi

# --- download (both tarballs fully, before touching the install dir) --------

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT INT TERM

for name in dobjd dobj; do
  url="$BASE/$name-$TARGET.tar.gz"
  say "  fetching $name-$TARGET.tar.gz ..."
  curl -fsSL --retry 3 -o "$TMP/$name.tar.gz" "$url" || err "download failed: $url
(if no release has been published yet, 'latest' does not resolve; set DOBJ_VERSION to a specific tag)"
  mkdir -p "$TMP/$name"
  tar -xzf "$TMP/$name.tar.gz" -C "$TMP/$name"
done

# --- install ----------------------------------------------------------------

mkdir -p "$BIN_DIR"

# A running daemon keeps working on the old inode; only a restart picks the
# new binaries up. The final step is a same-filesystem rename so the swap is
# atomic and never truncates a binary a process is executing.
PIDFILE="$HOME/.dobj/dobjd.pid"
RUNNING=""
if [ -f "$PIDFILE" ] && kill -0 "$(cat "$PIDFILE" 2>/dev/null)" 2>/dev/null; then
  RUNNING=yes
  say "  note: dobjd is running; restart it afterwards to use the new version"
fi

for name in dobjd dobj; do
  for src in "$TMP/$name"/*; do
    base=$(basename "$src")
    cp "$src" "$BIN_DIR/.$base.new"
    chmod +x "$BIN_DIR/.$base.new"
    mv -f "$BIN_DIR/.$base.new" "$BIN_DIR/$base"
    say "  installed $BIN_DIR/$base"
  done
done

# --- report + next steps ----------------------------------------------------

say ""
say "installed: $("$BIN_DIR/dobj" --version)"

case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *)
    say ""
    say "to use 'dobj' without the full path, add it to your PATH:"
    say "  echo 'export PATH=\"\$HOME/.dobj/bin:\$PATH\"' >> ~/.zshrc   # or ~/.bashrc"
    ;;
esac

say ""
if [ -n "$RUNNING" ]; then
  say "restart the daemon to pick up this version:"
  say "  $BIN_DIR/dobj stop && $BIN_DIR/dobj start"
else
  say "next step: start the daemon (the first start builds ZK circuits, ~2-5 min):"
  say "  $BIN_DIR/dobj start"
fi
