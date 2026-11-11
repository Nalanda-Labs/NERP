#!/usr/bin/env bash
# Point-in-time restore: the database as it was at a given moment, from a base backup made by
# nerp-backup.sh and the WAL archived by nerp-wal-archive.sh.
#
#   nerp-pitr-restore.sh --to "2026-11-12 10:30:00+01" [--env backend/.env] [--dest /var/backups/nerp]
#       [--target-db postgres://…/nerp_copy | --into-live --yes] [--keep] [--port 5499]
#
# It starts a temporary PostgreSQL server (only on a private socket) from the newest base
# backup taken before that moment, replays the archived changes up to it, and then:
# - with --target-db: copies that database into the given (existing, empty) database, e.g.
#   to look something up or to compare;
# - with --into-live --yes: replaces the server's own database (DATABASE_URL) with it, in one
#   transaction, and puts back files removed since then (kept aside in UPLOAD_DIR/.deleted).
#   Stop the NERP server first (or restore from the app, which pauses itself);
# - with neither: leaves the temporary server running and says how to connect (--keep too).
# The temporary server is stopped and removed at the end unless --keep is given.
# PostgreSQL's server programs of the same version are needed (PG_BIN, or the usual places).
set -euo pipefail

ENV_FILE="$(cd "$(dirname "$0")/../.." && pwd)/backend/.env"
DEST="/var/backups/nerp"
TO=""
TARGET_DB=""
LIVE=0
YES=0
KEEP=0
PORT=5499
while [ $# -gt 0 ]; do
  case "$1" in
    --to) TO="$2"; shift 2 ;;
    --env) ENV_FILE="$2"; shift 2 ;;
    --dest) DEST="$2"; shift 2 ;;
    --target-db) TARGET_DB="$2"; shift 2 ;;
    --into-live) LIVE=1; shift ;;
    --yes) YES=1; shift ;;
    --keep) KEEP=1; shift ;;
    --port) PORT="$2"; shift 2 ;;
    -h|--help) sed -n '2,21p' "$0"; exit 0 ;;
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
[ -n "$TO" ] || { echo "say which moment: --to \"YYYY-MM-DD HH:MM:SS+TZ\"" >&2; exit 2; }
if [ "$LIVE" = 1 ] && [ -n "$TARGET_DB" ]; then echo "either --into-live or --target-db" >&2; exit 2; fi
if [ "$LIVE" = 1 ] && [ "$YES" != 1 ]; then
  echo "This replaces the data in the server's own database." >&2
  echo "Stop the NERP server, then run again with --yes (or use --target-db for a copy)." >&2
  exit 1
fi
LIVE_URL="$(setting DATABASE_URL)"
[ -n "$LIVE_URL" ] || { echo "DATABASE_URL is not set (environment or $ENV_FILE)" >&2; exit 1; }
WAL_ARCHIVE_DIR="$(setting WAL_ARCHIVE_DIR)"
WAL_ARCHIVE_DIR="${WAL_ARCHIVE_DIR:-$DEST/wal}"
DBNAME="$(printf '%s' "$LIVE_URL" | sed -E 's#^[a-z]+://[^/]*/([^?]+).*#\1#')"
DBUSER="$(printf '%s' "$LIVE_URL" | sed -E 's#^[a-z]+://([^:@/]+).*#\1#')"

# The moment, in UTC, as PostgreSQL reads it (also checks that it can be read).
TO_UTC="$(psql "$LIVE_URL" -Atc "SELECT to_char(('$(printf '%s' "$TO" | sed "s/'/''/g")')::timestamptz AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS')")" \
  || { echo "can't read the moment \"$TO\"" >&2; exit 2; }
TO_STAMP="$(printf '%s' "$TO_UTC" | sed -E 's/^(....)-(..)-(..) (..):(..):(..)$/\1-\2-\3T\4\5\6Z/')"
NOW_STAMP="$(date -u +%Y-%m-%dT%H%M%SZ)"
[[ "$TO_STAMP" < "$NOW_STAMP" ]] || { echo "the moment ($TO_UTC UTC) is not in the past" >&2; exit 2; }

# The newest base backup from before the moment.
BACKUP=""
for b in $(find "$DEST" -mindepth 1 -maxdepth 1 -type d -name '20*' | sort -r); do
  name="$(basename "$b")"
  if [ -f "$b/base/base.tar.gz" ] && [[ "$name" < "$TO_STAMP" ]]; then BACKUP="$b"; break; fi
done
[ -n "$BACKUP" ] || { echo "no base backup from before $TO_UTC UTC in $DEST (they are made by nerp-backup.sh once WAL archiving is on)" >&2; exit 1; }
[ -d "$WAL_ARCHIVE_DIR" ] || { echo "no WAL archive at $WAL_ARCHIVE_DIR" >&2; exit 1; }

# The replay stops at the first change after the moment, and the changes still in the live
# server's current WAL segment aren't archived yet: make a change now and archive the
# segment (needs the right to call pg_switch_wal; without it, recent moments may not be
# reachable until the segment is archived on its own, at most archive_timeout later).
SEG="$(psql "$LIVE_URL" -Atc "SELECT txid_current()" -c "SELECT pg_walfile_name(pg_switch_wal())" 2>/dev/null | tail -1 || true)"
if [ -n "$SEG" ]; then
  for _ in $(seq 1 120); do
    { [ -f "$WAL_ARCHIVE_DIR/$SEG.gz" ] || [ -f "$WAL_ARCHIVE_DIR/$SEG" ]; } && break
    sleep 1
  done
fi

pg_tool() {
  local major
  major="$(sed -n 's/^\([0-9]*\).*/\1/p' "$WORKDIR/data/PG_VERSION")"
  for d in "$(setting PG_BIN)" "/usr/lib/postgresql/$major/bin" "/usr/pgsql-$major/bin" "$(pg_config --bindir 2>/dev/null || true)"; do
    [ -n "$d" ] && [ -x "$d/$1" ] && { echo "$d/$1"; return; }
  done
  command -v "$1" || { echo "PostgreSQL's $1 (version $major) was not found; set PG_BIN" >&2; exit 1; }
}

umask 077
WORKDIR="$DEST/.pitr-$NOW_STAMP"
mkdir -p "$WORKDIR/data" "$WORKDIR/socket"
chmod 700 "$WORKDIR" "$WORKDIR/data" "$WORKDIR/socket"
PG_CTL=""
stop_temp() {
  if [ -n "$PG_CTL" ] && [ -f "$WORKDIR/data/postmaster.pid" ]; then "$PG_CTL" -D "$WORKDIR/data" -m fast -w stop > /dev/null 2>&1 || true; fi
  rm -rf "$WORKDIR"
}
[ "$KEEP" = 1 ] || trap stop_temp EXIT

echo "restoring to $TO_UTC UTC from the base backup of $(basename "$BACKUP")…"
tar -xzf "$BACKUP/base/base.tar.gz" -C "$WORKDIR/data"
tar -xzf "$BACKUP/base/pg_wal.tar.gz" -C "$WORKDIR/data/pg_wal"
PG_CTL="$(pg_tool pg_ctl)"
# Only reachable on its private socket, without passwords; no archiving of its own.
cat > "$WORKDIR/data/pg_hba.conf" <<EOF
local all all trust
EOF
: > "$WORKDIR/data/pg_ident.conf"
# Servers set up by distributions keep postgresql.conf elsewhere (/etc): start from defaults.
[ -f "$WORKDIR/data/postgresql.conf" ] || : > "$WORKDIR/data/postgresql.conf"
rm -f "$WORKDIR/data/postmaster.pid" "$WORKDIR/data/postmaster.opts"
cat > "$WORKDIR/settings" <<EOF

# point-in-time restore ($NOW_STAMP)
listen_addresses = ''
port = $PORT
unix_socket_directories = '$WORKDIR/socket'
archive_mode = off
hba_file = '$WORKDIR/data/pg_hba.conf'
ident_file = '$WORKDIR/data/pg_ident.conf'
restore_command = 'f="$WAL_ARCHIVE_DIR/%f"; if [ -f "\$f.gz" ]; then gzip -dc "\$f.gz" > "%p"; else cp "\$f" "%p"; fi'
recovery_target_time = '$TO_UTC+00'
recovery_target_action = 'promote'
recovery_target_inclusive = true
EOF
# A base backup carries settings of the server it came from; some may not fit here.
for f in postgresql.conf postgresql.auto.conf; do
  sed -i -E '/^[[:space:]]*(shared_preload_libraries|ssl|ssl_[a-z_]+|include|include_dir|include_if_exists|data_directory|config_file|external_pid_file|hba_file|ident_file|listen_addresses|port|unix_socket_directories|archive_mode|archive_command|restore_command|recovery_target[a-z_]*)[[:space:]]*=/d' "$WORKDIR/data/$f"
done
cat "$WORKDIR/settings" >> "$WORKDIR/data/postgresql.auto.conf"
touch "$WORKDIR/data/recovery.signal"
chmod -R go-rwx "$WORKDIR/data"

"$PG_CTL" -D "$WORKDIR/data" -l "$WORKDIR/server.log" -w -t 3600 start > /dev/null \
  || { cat "$WORKDIR/server.log" >&2 || true; exit 1; }
TEMP_URL="postgresql:///$DBNAME?host=$WORKDIR/socket&port=$PORT&user=$DBUSER"
# Replaying ends with a promotion: until then the server is still "in recovery".
for _ in $(seq 1 3600); do
  [ "$(psql "$TEMP_URL" -Atc 'SELECT pg_is_in_recovery()' 2>/dev/null)" = f ] && break
  sleep 1
done
[ "$(psql "$TEMP_URL" -Atc 'SELECT pg_is_in_recovery()')" = f ] || { tail -20 "$WORKDIR/server.log" >&2; echo "the replay did not finish" >&2; exit 1; }
if ! grep -q "recovery stopping before\|recovery stopping after\|recovery target" "$WORKDIR/server.log"; then
  echo "note: the archive ends before $TO_UTC UTC; restored to its last change" >&2
fi
LAST="$(grep -o 'last completed transaction was at log time [^"]*' "$WORKDIR/server.log" | tail -1 | sed 's/.*log time //')"
echo "replayed up to $TO_UTC UTC${LAST:+ (last change restored: $LAST)}"

if [ -n "$TARGET_DB" ] || [ "$LIVE" = 1 ]; then
  [ -n "$TARGET_DB" ] || TARGET_DB="$LIVE_URL"
  if [ "$LIVE" = 1 ]; then echo "copying it into the server's own database…"; else echo "copying it into the other database…"; fi
  "$(pg_tool pg_dump)" --format=custom --no-owner --no-privileges --file="$WORKDIR/at-moment.dump" "$TEMP_URL"
  # One transaction: either everything is replaced or nothing changes.
  "$(pg_tool pg_restore)" --clean --if-exists --no-owner --no-privileges --single-transaction --exit-on-error --dbname="$TARGET_DB" "$WORKDIR/at-moment.dump"
  if [ "$LIVE" = 1 ]; then
    UPLOAD_DIR="$(setting UPLOAD_DIR)"
    UPLOAD_DIR="${UPLOAD_DIR:-./data/uploads}"
    case "$UPLOAD_DIR" in /*) ;; *) UPLOAD_DIR="$(dirname "$ENV_FILE")/${UPLOAD_DIR#./}" ;; esac
    # Files removed since the moment are put back where the restored database expects them.
    DAY="${TO_UTC:0:10}"
    n=0
    if [ -d "$UPLOAD_DIR/.deleted" ]; then
      for d in "$UPLOAD_DIR"/.deleted/*/; do
        [ -d "$d" ] || continue
        [[ "$(basename "$d")" < "$DAY" ]] && continue
        rsync -a --ignore-existing "$d" "$UPLOAD_DIR/"
        n=$((n + 1))
      done
    fi
    echo "put back files removed since then (from $n day(s) kept aside)"
  fi
  echo "restored the database as it was at $TO_UTC UTC"
else
  trap - EXIT
  echo "the database as it was is running on the socket $WORKDIR/socket, port $PORT:"
  echo "  psql \"$TEMP_URL\""
  echo "stop it with: $PG_CTL -D $WORKDIR/data -m fast stop && rm -rf $WORKDIR"
fi
