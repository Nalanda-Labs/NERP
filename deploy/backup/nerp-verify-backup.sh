#!/usr/bin/env bash
# Proves the newest backup can be restored: restores it into a temporary database next to
# the live one, checks it, and drops it again. Run it after every backup (the systemd timer
# does): a backup that has never been restored is only a hope.
#
#   nerp-verify-backup.sh [--env backend/.env] [--dest /var/backups/nerp]
#
# Fails (exit 1) when the checksum is wrong, the restore fails, a table or migration is
# missing, or there are no people in it. Row counts are compared with the live database
# for information (the live data has moved on since the backup).
# The database user needs the CREATEDB right (or be the owner of the server).
set -euo pipefail

ENV_FILE="$(cd "$(dirname "$0")/../.." && pwd)/backend/.env"
DEST="/var/backups/nerp"
while [ $# -gt 0 ]; do
  case "$1" in
    --env) ENV_FILE="$2"; shift 2 ;;
    --dest) DEST="$2"; shift 2 ;;
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
LIVE_URL="$(setting DATABASE_URL)"
[ -n "$LIVE_URL" ] || { echo "DATABASE_URL is not set" >&2; exit 1; }
BACKUP="$(find "$DEST" -mindepth 1 -maxdepth 1 -type d -name '20*' | sort | tail -1)"
[ -n "$BACKUP" ] || { echo "no backups in $DEST" >&2; exit 1; }

# The same server, another database name: postgres://u:p@host:port/NAME?options
SCRATCH="nerp_verify_$(date -u +%Y%m%d%H%M%S)"
SCRATCH_URL="$(printf '%s' "$LIVE_URL" | sed -E "s#/[^/?]+(\?|$)#/$SCRATCH\1#")"
ADMIN_URL="$(printf '%s' "$LIVE_URL" | sed -E "s#/[^/?]+(\?|$)#/postgres\1#")"
cleanup() { psql "$ADMIN_URL" -qAtc "DROP DATABASE IF EXISTS $SCRATCH" >/dev/null 2>&1 || true; }
trap cleanup EXIT

echo "verifying $BACKUP"
psql "$ADMIN_URL" -qAtc "CREATE DATABASE $SCRATCH"
"$(dirname "$0")/nerp-restore.sh" "$BACKUP" --env "$ENV_FILE" --target-db "$SCRATCH_URL" > /dev/null

fail=0
tables() { psql "$1" -Atc "SELECT tablename FROM pg_tables WHERE schemaname = 'public' ORDER BY 1"; }
missing="$(comm -23 <(tables "$LIVE_URL") <(tables "$SCRATCH_URL"))"
if [ -n "$missing" ]; then echo "FAIL tables missing from the backup: $missing"; fail=1; fi
restored_migrations="$(psql "$SCRATCH_URL" -Atc "SELECT version, description FROM _sqlx_migrations ORDER BY version")"
if [ "$restored_migrations" != "$(cat "$BACKUP/migrations.txt")" ]; then echo "FAIL the migrations differ from the backup's list"; fail=1; fi
people="$(psql "$SCRATCH_URL" -Atc "SELECT count(*) FROM users WHERE NOT is_system")"
if [ "${people:-0}" -lt 1 ]; then echo "FAIL no people in the restored database"; fail=1; fi

count_all() {
  psql "$1" -Atc "$(psql "$1" -Atc "SELECT string_agg(format('SELECT %L, count(*) FROM %I', tablename, tablename), ' UNION ALL ') FROM pg_tables WHERE schemaname = 'public'") ORDER BY 1"
}
changed="$(diff <(count_all "$SCRATCH_URL") <(count_all "$LIVE_URL") | grep -c '^>' || true)"
total_rows="$(count_all "$SCRATCH_URL" | awk -F'|' '{s += $2} END {print s}')"
echo "restored $(tables "$SCRATCH_URL" | wc -l) tables, $total_rows rows, $people people; $changed tables changed in the live database since"

# The result goes into the backup's manifest, where System → Health reads it.
if [ "$fail" = 0 ]; then
  echo "verified_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)" >> "$BACKUP/manifest.txt" 2>/dev/null || true
  echo "backup OK"
else
  echo "verify_failed_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)" >> "$BACKUP/manifest.txt" 2>/dev/null || true
  echo "backup FAILED verification"
  exit 1
fi
