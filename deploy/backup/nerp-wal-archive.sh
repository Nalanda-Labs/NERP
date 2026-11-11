#!/usr/bin/env bash
# PostgreSQL's archive_command for point-in-time restores: keeps every finished WAL segment
# (the database's change log), compressed, in the archive folder.
#
# In postgresql.conf (then restart PostgreSQL):
#   wal_level = replica
#   archive_mode = on
#   archive_command = '/opt/nerp/deploy/backup/nerp-wal-archive.sh %p %f /var/backups/nerp/wal'
#   archive_timeout = 300        # at most 5 minutes of changes wait for a full segment
#
# The folder must be writable by the postgres user. nerp-backup.sh takes the base backups
# that the archive starts from, and removes segments older than the oldest base backup kept.
# Exit 0 only once the segment is safely on disk; PostgreSQL retries until then.
set -euo pipefail
SRC="${1:?usage: nerp-wal-archive.sh %p %f ARCHIVE_DIR}"
NAME="${2:?usage: nerp-wal-archive.sh %p %f ARCHIVE_DIR}"
DIR="${3:-/var/backups/nerp/wal}"
umask 077
mkdir -p "$DIR"
OUT="$DIR/$NAME.gz"
if [ -f "$OUT" ]; then
  # Already archived (PostgreSQL retries after a crash): fine if it is the same segment.
  if gzip -dc "$OUT" | cmp -s - "$SRC"; then exit 0; fi
  echo "nerp-wal-archive: $OUT exists with other contents; not overwriting it" >&2
  exit 1
fi
TMP="$DIR/.$NAME.gz.$$"
trap 'rm -f "$TMP"' EXIT
gzip -1 -c "$SRC" > "$TMP"
sync "$TMP"
mv "$TMP" "$OUT"
sync "$DIR"
trap - EXIT
