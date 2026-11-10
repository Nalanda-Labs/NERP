#!/usr/bin/env bash
# Restores a backup made by nerp-backup.sh.
#
#   nerp-restore.sh /var/backups/nerp/2026-11-10T020000Z [--env backend/.env]
#                   [--target-db postgres://…/nerp_copy] [--uploads-to /path] [--yes]
#
# By default it restores into the server's own database (DATABASE_URL) and upload folder
# (UPLOAD_DIR), replacing what is there; that needs --yes, and the NERP server must be
# stopped first. With --target-db (an existing, empty database) it restores a copy next
# to the live one instead, e.g. to look something up or to test the backup.
set -euo pipefail

BACKUP="${1:-}"
[ -n "$BACKUP" ] && [ -d "$BACKUP" ] || { echo "usage: $0 BACKUP_FOLDER [--env FILE] [--target-db URL] [--uploads-to DIR] [--yes]" >&2; exit 2; }
shift
ENV_FILE="$(cd "$(dirname "$0")/../.." && pwd)/backend/.env"
TARGET_DB=""
UPLOADS_TO=""
YES=0
while [ $# -gt 0 ]; do
  case "$1" in
    --env) ENV_FILE="$2"; shift 2 ;;
    --target-db) TARGET_DB="$2"; shift 2 ;;
    --uploads-to) UPLOADS_TO="$2"; shift 2 ;;
    --yes) YES=1; shift ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
done
setting() {
  local name="$1" value="${!1:-}"
  if [ -z "$value" ] && [ -f "$ENV_FILE" ]; then
    value="$(grep -E "^${name}=" "$ENV_FILE" | tail -1 | cut -d= -f2- | sed -e 's/^"//' -e 's/"$//')"
  fi
  printf '%s' "$value"
}

echo "checking the backup…"
(cd "$BACKUP" && sha256sum --quiet -c database.dump.sha256)
pg_restore --list "$BACKUP/database.dump" > /dev/null

LIVE=0
if [ -z "$TARGET_DB" ]; then
  TARGET_DB="$(setting DATABASE_URL)"
  LIVE=1
fi
[ -n "$TARGET_DB" ] || { echo "no database to restore into (DATABASE_URL or --target-db)" >&2; exit 1; }
if [ "$LIVE" = 1 ] && [ "$YES" != 1 ]; then
  echo "This replaces the data in the server's own database and upload folder." >&2
  echo "Stop the NERP server, then run again with --yes (or use --target-db for a copy)." >&2
  exit 1
fi

echo "restoring the database…"
# One transaction: either the whole backup is restored or nothing changes.
pg_restore --clean --if-exists --no-owner --no-privileges --single-transaction --exit-on-error --dbname="$TARGET_DB" "$BACKUP/database.dump"

if [ -z "$UPLOADS_TO" ] && [ "$LIVE" = 1 ]; then
  UPLOADS_TO="$(setting UPLOAD_DIR)"
  UPLOADS_TO="${UPLOADS_TO:-./data/uploads}"
  case "$UPLOADS_TO" in /*) ;; *) UPLOADS_TO="$(dirname "$ENV_FILE")/${UPLOADS_TO#./}" ;; esac
fi
if [ -n "$UPLOADS_TO" ]; then
  echo "restoring uploaded files to $UPLOADS_TO…"
  mkdir -p "$UPLOADS_TO"
  rsync -a --delete "$BACKUP/uploads/" "$UPLOADS_TO/"
fi
echo "restored $(grep created_at "$BACKUP/manifest.txt" | cut -d= -f2) (last migration $(grep last_migration "$BACKUP/manifest.txt" | cut -d= -f2))"
