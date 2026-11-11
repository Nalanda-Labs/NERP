#!/usr/bin/env bash
# Point-in-time restores: streams the database's change log (WAL) from the PostgreSQL server —
# on this machine or another one — into the archive folder here, as it is written. Runs all the
# time (deploy/systemd/nerp-wal-receive.service); nothing is installed on the database server.
#
#   nerp-wal-receive.sh [--env /path/to/backend/.env] [--dest /var/backups/nerp]
#
# It uses a replication slot (WAL_SLOT, default nerp_wal), made on first start, so the server
# keeps changes until they have been received, even while this is stopped. Watch it: a slot
# nobody reads makes the server keep WAL until its disk fills (cap it with
# max_slot_wal_keep_size on the server; System → Backups warns when receiving stops).
# The database user needs the REPLICATION right and a "replication" line in pg_hba.conf.
# Finished segments are compressed by nerp-backup.sh; the one being written ends in .partial.
set -euo pipefail

ENV_FILE="$(cd "$(dirname "$0")/../.." && pwd)/backend/.env"
DEST="/var/backups/nerp"
while [ $# -gt 0 ]; do
  case "$1" in
    --env) ENV_FILE="$2"; shift 2 ;;
    --dest) DEST="$2"; shift 2 ;;
    -h|--help) sed -n '2,15p' "$0"; exit 0 ;;
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
DATABASE_URL="$(setting DATABASE_URL)"
[ -n "$DATABASE_URL" ] || { echo "DATABASE_URL is not set (environment or $ENV_FILE)" >&2; exit 1; }
WAL_ARCHIVE_DIR="$(setting WAL_ARCHIVE_DIR)"
WAL_ARCHIVE_DIR="${WAL_ARCHIVE_DIR:-$DEST/wal}"
SLOT="$(setting WAL_SLOT)"
SLOT="${SLOT:-nerp_wal}"

# pg_receivewal of the server's version (client programs; PG_BIN overrides the search).
MAJOR="$(psql "$DATABASE_URL" -Atc "SELECT current_setting('server_version_num')::int / 10000")"
RECEIVE=""
for d in "${PG_BIN:-}" "/usr/lib/postgresql/$MAJOR/bin" "/usr/pgsql-$MAJOR/bin" "$(pg_config --bindir 2>/dev/null || true)"; do
  [ -n "$d" ] && [ -x "$d/pg_receivewal" ] && { RECEIVE="$d/pg_receivewal"; break; }
done
RECEIVE="${RECEIVE:-$(command -v pg_receivewal || true)}"
[ -n "$RECEIVE" ] || { echo "pg_receivewal (PostgreSQL $MAJOR client programs) was not found" >&2; exit 1; }

umask 077
mkdir -p "$WAL_ARCHIVE_DIR"
exec 9>"$WAL_ARCHIVE_DIR/.receive.lock"
flock -n 9 || { echo "another nerp-wal-receive.sh is running for $WAL_ARCHIVE_DIR" >&2; exit 1; }
"$RECEIVE" --dbname="$DATABASE_URL" --slot="$SLOT" --create-slot --if-not-exists --no-password
echo "receiving WAL into $WAL_ARCHIVE_DIR (slot $SLOT)"
# --synchronous: each change is on disk here as soon as the server sends it.
exec "$RECEIVE" --dbname="$DATABASE_URL" --directory="$WAL_ARCHIVE_DIR" --slot="$SLOT" --synchronous --no-password
