#!/usr/bin/env bash
set -euo pipefail

APP_USER="zkcraft"
APP_GROUP="zkcraft"
STATE_DIR="/var/lib/zkcraft"
DB_ROLE="zkcraft"
SYNC_DB_NAME="synchronizer"
RELAYER_DB_NAME="relayer"
SYNC_STATE_DB_PATH="$STATE_DIR/synchronizer-db"

log() {
  printf '[zkcraft-reset-db] %s\n' "$*"
}

fail() {
  printf '[zkcraft-reset-db] ERROR: %s\n' "$*" >&2
  exit 1
}

require_root() {
  if [[ "${EUID}" -ne 0 ]]; then
    fail "run this script as root"
  fi
}

require_app_user() {
  if ! id -u "$APP_USER" >/dev/null 2>&1; then
    fail "missing system user $APP_USER; run bootstrap first"
  fi
}

require_db_role() {
  if ! runuser -u postgres -- psql -tAc "SELECT 1 FROM pg_roles WHERE rolname = '$DB_ROLE'" | grep -q 1; then
    fail "missing Postgres role $DB_ROLE; run bootstrap first"
  fi
}

ensure_postgres_running() {
  log "ensuring PostgreSQL is running"
  systemctl enable --now postgresql
}

service_exists() {
  systemctl cat "$1" >/dev/null 2>&1
}

stop_service_if_present() {
  local service="$1"
  if service_exists "$service"; then
    log "stopping $service"
    systemctl stop "$service"
  fi
}

restart_service_if_present() {
  local service="$1"
  if service_exists "$service"; then
    log "restarting $service"
    systemctl restart "$service"
  fi
}

recreate_database() {
  local database_name="$1"
  log "recreating Postgres database $database_name"
  runuser -u postgres -- dropdb --if-exists "$database_name"
  runuser -u postgres -- createdb --owner "$DB_ROLE" "$database_name"
}

reset_sync_state() {
  log "resetting synchronizer state at $SYNC_STATE_DB_PATH"
  rm -rf "$SYNC_STATE_DB_PATH"
  install -d -o "$APP_USER" -g "$APP_GROUP" "$SYNC_STATE_DB_PATH"
}

main() {
  require_root
  require_app_user
  ensure_postgres_running
  require_db_role

  stop_service_if_present synchronizer.service
  stop_service_if_present relayer.service

  recreate_database "$SYNC_DB_NAME"
  recreate_database "$RELAYER_DB_NAME"
  reset_sync_state

  restart_service_if_present synchronizer.service
  restart_service_if_present relayer.service

  log "reset complete"
}

main
