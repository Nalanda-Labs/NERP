#!/usr/bin/env bash
# NERP backup: the database and the uploaded files, into one dated folder.
#
#   nerp-backup.sh [--env /path/to/backend/.env] [--dest /var/backups/nerp] [--keep-days 14]
#
# - Database: `pg_dump --format=custom` (compressed, restorable table by table), plus its
#   checksum and the list of applied migrations.
# - Files: the upload folder, copied with rsync. Unchanged files are hard links to the
#   previous backup, so every backup is complete but only new files take space.
# - Point-in-time restores: when nerp-wal-receive.sh streams the database's WAL into
#   WAL_ARCHIVE_DIR (default DEST/wal; its replication slot WAL_SLOT exists), also a base
#   backup of the database server (pg_basebackup, over the network like pg_dump); finished
#   WAL segments are compressed, and those older than the oldest base backup kept are
#   removed. Restore to any moment since the oldest base backup with nerp-pitr-restore.sh.
#   The database user needs the REPLICATION right.
# - Backups older than --keep-days are removed.
# - Settings come from backend/.env (DATABASE_URL, UPLOAD_DIR), like the server's.
#
# Not included, on purpose: Redis (sessions and caches; people just sign in again) and
# backend/.env itself. Keep a copy of .env somewhere safe and separate: without its
# JWT_SECRET and DATA_ENCRYPTION_KEY, mailbox passwords and other stored secrets in the
# database cannot be read.
set -euo pipefail

ENV_FILE="$(cd "$(dirname "$0")/../.." && pwd)/backend/.env"
DEST="/var/backups/nerp"
KEEP_DAYS=14
while [ $# -gt 0 ]; do
  case "$1" in
    --env) ENV_FILE="$2"; shift 2 ;;
    --dest) DEST="$2"; shift 2 ;;
    --keep-days) KEEP_DAYS="$2"; shift 2 ;;
    -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
done

# Read one setting from the environment, else from the .env file (without running it).
setting() {
  local name="$1" value="${!1:-}"
  if [ -z "$value" ] && [ -f "$ENV_FILE" ]; then
    value="$(grep -E "^${name}=" "$ENV_FILE" | tail -1 | cut -d= -f2- | sed -e 's/^"//' -e 's/"$//')"
  fi
  printf '%s' "$value"
}
DATABASE_URL="$(setting DATABASE_URL)"
UPLOAD_DIR="$(setting UPLOAD_DIR)"
[ -n "$DATABASE_URL" ] || { echo "DATABASE_URL is not set (environment or $ENV_FILE)" >&2; exit 1; }
UPLOAD_DIR="${UPLOAD_DIR:-./data/uploads}"
# A relative UPLOAD_DIR is relative to backend/, where the server runs.
case "$UPLOAD_DIR" in /*) ;; *) UPLOAD_DIR="$(dirname "$ENV_FILE")/${UPLOAD_DIR#./}" ;; esac

umask 077
mkdir -p "$DEST"
exec 9>"$DEST/.lock"
flock -n 9 || { echo "another backup is running" >&2; exit 1; }

STAMP="$(date -u +%Y-%m-%dT%H%M%SZ)"
WORK="$DEST/.in-progress-$STAMP"
mkdir -p "$WORK"
trap 'rm -rf "$WORK"' EXIT

echo "backing up the database…"
pg_dump --format=custom --compress=6 --no-owner --no-privileges --file="$WORK/database.dump" "$DATABASE_URL"
sha256sum "$WORK/database.dump" | sed "s|$WORK/||" > "$WORK/database.dump.sha256"
psql "$DATABASE_URL" -Atc "SELECT version, description FROM _sqlx_migrations ORDER BY version" > "$WORK/migrations.txt"
psql "$DATABASE_URL" -Atc "SELECT relname, n_live_tup FROM pg_stat_user_tables ORDER BY relname" > "$WORK/table-sizes.txt"

# PostgreSQL's client programs of the server's version: the usual places, else PATH (PG_BIN,
# from the environment, overrides the search for unusual layouts).
pg_tool() {
  local major
  major="$(psql "$DATABASE_URL" -Atc "SELECT current_setting('server_version_num')::int / 10000")"
  for d in "${PG_BIN:-}" "/usr/lib/postgresql/$major/bin" "/usr/pgsql-$major/bin" "$(pg_config --bindir 2>/dev/null || true)"; do
    [ -n "$d" ] && [ -x "$d/$1" ] && { echo "$d/$1"; return; }
  done
  command -v "$1"
}
WAL_ARCHIVE_DIR="$(setting WAL_ARCHIVE_DIR)"
WAL_ARCHIVE_DIR="${WAL_ARCHIVE_DIR:-$DEST/wal}"
WAL_SLOT="$(setting WAL_SLOT)"
WAL_SLOT="${WAL_SLOT:-nerp_wal}"
RECEIVING="$(psql "$DATABASE_URL" -Atc "SELECT count(*) FROM pg_replication_slots WHERE slot_name = '$WAL_SLOT'")"
BASE=0
if [ "$RECEIVING" = 1 ] && [ -d "$WAL_ARCHIVE_DIR" ]; then
  echo "taking a base backup for point-in-time restores…"
  "$(pg_tool pg_basebackup)" --dbname="$DATABASE_URL" --pgdata="$WORK/base" --format=tar --gzip --wal-method=stream --checkpoint=fast --no-password
  BASE=1
  BASE_WAL="$(tar -xzOf "$WORK/base/base.tar.gz" backup_label | sed -n 's/^START WAL LOCATION: .*(file \(.*\))$/\1/p')"
fi

echo "backing up uploaded files from $UPLOAD_DIR…"
PREVIOUS="$(find "$DEST" -mindepth 1 -maxdepth 1 -type d -name '20*' | sort | tail -1)"
mkdir -p "$WORK/uploads"
if [ -d "$UPLOAD_DIR" ]; then
  rsync -a --delete ${PREVIOUS:+--link-dest="$PREVIOUS/uploads"} "$UPLOAD_DIR/" "$WORK/uploads/"
fi

{
  echo "created_at=$STAMP"
  echo "host=$(hostname)"
  echo "database_bytes=$(stat -c %s "$WORK/database.dump")"
  echo "upload_files=$(find "$WORK/uploads" -type f | wc -l)"
  echo "last_migration=$(tail -1 "$WORK/migrations.txt" | cut -d'|' -f1)"
  if [ "$BASE" = 1 ]; then
    echo "base_backup=1"
    echo "base_wal=$BASE_WAL"
  fi
} > "$WORK/manifest.txt"

mv "$WORK" "$DEST/$STAMP"
trap - EXIT
echo "backup written to $DEST/$STAMP"

# Retention: remove backups older than KEEP_DAYS (the one just written is never among them).
find "$DEST" -mindepth 1 -maxdepth 1 -type d -name '20*' -mtime +"$KEEP_DAYS" | sort | while read -r old; do
  echo "removing old backup $old"
  rm -rf -- "$old"
done

# The WAL archive: finished segments compressed (the one being received ends in .partial);
# segments from before the oldest base backup kept can't be used any more.
if [ -d "$WAL_ARCHIVE_DIR" ]; then
  find "$WAL_ARCHIVE_DIR" -maxdepth 1 -type f -regextype posix-extended -regex '.*/[0-9A-F]{24}' -print0 | xargs -0 -r gzip -1
  OLDEST_WAL="$(find "$DEST" -mindepth 2 -maxdepth 2 -name manifest.txt -path "$DEST/20*" | sort | xargs -r grep -h '^base_wal=' | head -1 | cut -d= -f2 || true)"
  if [ -n "$OLDEST_WAL" ]; then
    # Same timeline or earlier, and an earlier position (names sort by timeline, then position).
    find "$WAL_ARCHIVE_DIR" -maxdepth 1 -type f -regextype posix-extended -regex '.*/[0-9A-F]{24}(\.gz)?' | while read -r f; do
      n="$(basename "$f" .gz)"
      if [[ "${n:0:8}" < "${OLDEST_WAL:0:8}" || ( "${n:0:8}" == "${OLDEST_WAL:0:8}" && "${n:8}" < "${OLDEST_WAL:8}" ) ]]; then rm -f -- "$f"; fi
    done
  fi
fi
