#!/usr/bin/env bash
set -euo pipefail

APP_USER="zkcraft"
APP_GROUP="zkcraft"
APP_HOME="/srv/zkcraft"
INSTALL_DIR="$APP_HOME/repo"
ETC_DIR="/etc/zkcraft"
STATE_DIR="/var/lib/zkcraft"
SYSTEMD_DIR="/etc/systemd/system"
RUST_TOOLCHAIN="nightly-2026-01-25"
DB_ROLE="zkcraft"
SYNC_DB_NAME="synchronizer"
RELAYER_DB_NAME="relayer"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

log() {
  printf '[zkcraft-bootstrap] %s\n' "$*"
}

fail() {
  printf '[zkcraft-bootstrap] ERROR: %s\n' "$*" >&2
  exit 1
}

require_root() {
  if [[ "${EUID}" -ne 0 ]]; then
    fail "run this script as root"
  fi
}

require_debian_13() {
  if [[ ! -r /etc/os-release ]]; then
    fail "cannot detect OS: /etc/os-release is missing"
  fi

  # shellcheck disable=SC1091
  . /etc/os-release

  if [[ "${ID:-}" != "debian" || "${VERSION_ID:-}" != "13" ]]; then
    fail "expected Debian 13, got ${PRETTY_NAME:-unknown}"
  fi
}

install_packages() {
  log "installing system packages"
  export DEBIAN_FRONTEND=noninteractive
  apt-get update
  apt-get install -y \
    build-essential \
    clang \
    libclang-dev \
    cmake \
    pkg-config \
    libssl-dev \
    openssl \
    git \
    curl \
    ca-certificates \
    rsync \
    postgresql \
    postgresql-contrib \
    libgfortran5 \
    libquadmath0 \
    libgomp1
}

ensure_user() {
  if ! id -u "$APP_USER" >/dev/null 2>&1; then
    log "creating system user $APP_USER"
    adduser --system --group --home "$APP_HOME" "$APP_USER"
  fi

  mkdir -p "$APP_HOME" "$INSTALL_DIR" "$ETC_DIR" "$STATE_DIR"
  chown -R "$APP_USER:$APP_GROUP" "$APP_HOME" "$STATE_DIR"
  chmod 700 "$ETC_DIR"
}

sync_repo() {
  log "syncing repository into $INSTALL_DIR"
  rsync -a \
    --exclude '.git' \
    --exclude 'target' \
    --exclude 'app-gui/node_modules' \
    "$REPO_ROOT"/ "$INSTALL_DIR"/
  chown -R "$APP_USER:$APP_GROUP" "$INSTALL_DIR"
}

run_as_app_user() {
  su -s /bin/bash "$APP_USER" -c "$1"
}

install_rust() {
  if [[ ! -x "$APP_HOME/.cargo/bin/rustup" ]]; then
    log "installing rustup for $APP_USER"
    run_as_app_user "curl https://sh.rustup.rs -sSf | sh -s -- -y"
  fi

  log "installing Rust toolchain $RUST_TOOLCHAIN"
  run_as_app_user "source '$APP_HOME/.cargo/env' && rustup toolchain install '$RUST_TOOLCHAIN'"
}

build_binaries() {
  local jobs="${CARGO_BUILD_JOBS:-1}"
  log "building synchronizer and relayer with CARGO_BUILD_JOBS=$jobs"
  run_as_app_user "
    source '$APP_HOME/.cargo/env' &&
    cd '$INSTALL_DIR' &&
    CARGO_BUILD_JOBS='$jobs' cargo build --release -p synchronizer -p relayer
  "
}

ensure_postgres_running() {
  log "enabling PostgreSQL"
  systemctl enable --now postgresql
}

ensure_db_password() {
  local password_file="$ETC_DIR/db-password"
  if [[ ! -f "$password_file" ]]; then
    log "generating local Postgres password"
    openssl rand -hex 24 >"$password_file"
    chmod 600 "$password_file"
  fi
}

configure_postgres() {
  local db_password
  db_password="$(cat "$ETC_DIR/db-password")"

  log "configuring local Postgres role and databases"
  if ! runuser -u postgres -- psql -tAc "SELECT 1 FROM pg_roles WHERE rolname = '$DB_ROLE'" | grep -q 1; then
    runuser -u postgres -- createuser --login "$DB_ROLE"
  fi

  runuser -u postgres -- psql -v ON_ERROR_STOP=1 -d postgres \
    -c "ALTER ROLE \"$DB_ROLE\" WITH PASSWORD '$db_password';"

  if ! runuser -u postgres -- psql -tAc "SELECT 1 FROM pg_database WHERE datname = '$SYNC_DB_NAME'" | grep -q 1; then
    runuser -u postgres -- createdb --owner "$DB_ROLE" "$SYNC_DB_NAME"
  fi

  if ! runuser -u postgres -- psql -tAc "SELECT 1 FROM pg_database WHERE datname = '$RELAYER_DB_NAME'" | grep -q 1; then
    runuser -u postgres -- createdb --owner "$DB_ROLE" "$RELAYER_DB_NAME"
  fi
}

install_env_file() {
  local source_file="$1"
  local target_file="$2"

  if [[ ! -f "$target_file" ]]; then
    log "installing $(basename "$target_file")"
    install -m 600 "$source_file" "$target_file"
  fi
}

render_env_templates() {
  local db_password
  db_password="$(cat "$ETC_DIR/db-password")"

  sed \
    -e "s|__SYNC_METADATA_DB_URL__|postgres://$DB_ROLE:$db_password@127.0.0.1:5432/$SYNC_DB_NAME|g" \
    -e "s|__APP_STATE_DB_PATH__|$STATE_DIR/synchronizer-db|g" \
    "$INSTALL_DIR/deploy/ec2/synchronizer.env.example" >"$ETC_DIR/synchronizer.env.rendered"

  sed \
    -e "s|__DB_URL__|postgres://$DB_ROLE:$db_password@127.0.0.1:5432/$RELAYER_DB_NAME|g" \
    "$INSTALL_DIR/deploy/ec2/relayer.env.example" >"$ETC_DIR/relayer.env.rendered"

  install_env_file "$ETC_DIR/synchronizer.env.rendered" "$ETC_DIR/synchronizer.env"
  install_env_file "$ETC_DIR/relayer.env.rendered" "$ETC_DIR/relayer.env"

  rm -f "$ETC_DIR/synchronizer.env.rendered" "$ETC_DIR/relayer.env.rendered"
}

install_unit() {
  local source_file="$1"
  local target_file="$2"
  log "installing $(basename "$target_file")"
  install -m 644 "$source_file" "$target_file"
}

install_systemd_units() {
  install_unit \
    "$INSTALL_DIR/deploy/systemd/synchronizer.service" \
    "$SYSTEMD_DIR/synchronizer.service"
  install_unit \
    "$INSTALL_DIR/deploy/systemd/relayer.service" \
    "$SYSTEMD_DIR/relayer.service"
  systemctl daemon-reload
}

env_is_configured() {
  local env_file="$1"
  ! grep -q 'REPLACE_ME' "$env_file"
}

enable_and_start_services_if_ready() {
  systemctl enable synchronizer.service relayer.service

  if env_is_configured "$ETC_DIR/synchronizer.env" && env_is_configured "$ETC_DIR/relayer.env"; then
    log "starting services"
    systemctl restart synchronizer.service relayer.service
  else
    log "env files still contain placeholders; services were installed but not started"
  fi
}

print_next_steps() {
  cat <<EOF

Bootstrap complete.

Files:
  Repo:                 $INSTALL_DIR
  Synchronizer env:     $ETC_DIR/synchronizer.env
  Relayer env:          $ETC_DIR/relayer.env
  Synchronizer unit:    $SYSTEMD_DIR/synchronizer.service
  Relayer unit:         $SYSTEMD_DIR/relayer.service

Next steps:
  1. Edit the placeholder values in:
       sudo editor $ETC_DIR/synchronizer.env
       sudo editor $ETC_DIR/relayer.env
  2. Start services:
       sudo systemctl restart synchronizer relayer
  3. Check logs:
       journalctl -u synchronizer -f
       journalctl -u relayer -f
EOF
}

main() {
  require_root
  require_debian_13
  install_packages
  ensure_user
  sync_repo
  ensure_postgres_running
  ensure_db_password
  configure_postgres
  install_rust
  build_binaries
  render_env_templates
  install_systemd_units
  enable_and_start_services_if_ready
  print_next_steps
}

main "$@"
