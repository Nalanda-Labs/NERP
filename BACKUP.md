# Backups

What to back up, how NERP's backups work, and how to restore — from one deleted record to a
lost server.

## What holds NERP's data

| What | Where | Backed up by |
|------|-------|--------------|
| The database (everything people enter) | PostgreSQL, `DATABASE_URL` | nightly dump + continuous WAL archive |
| Uploaded files (attachments, documents, avatars, drive) | `UPLOAD_DIR` (`backend/data/uploads`) | nightly snapshot |
| Secrets: `JWT_SECRET`, `DATA_ENCRYPTION_KEY`, SMTP and OAuth secrets | `backend/.env` | by hand, kept apart (see below) |
| Call recordings, if LiveKit recording is on | `LIVEKIT_RECORDINGS_DIR` | add it to the file snapshot |
| Sessions, rate limits, caches | Redis / Valkey | not needed: people sign in again |

**The secrets matter as much as the data.** Mailbox passwords and OAuth tokens are stored
encrypted with `DATA_ENCRYPTION_KEY` (or a key derived from `JWT_SECRET`). A database restored
without the same `.env` works, but every connected mailbox has to be connected again. Keep a
copy of `backend/.env` in the company's password manager or a sealed vault, never next to the
backups.

## Goals

- **Recovery point:** at most a few minutes of work lost (continuous WAL archiving); at most
  one day if only the nightly backups survive.
- **Recovery time:** about an hour to bring NERP back on a new machine from these notes.
- **3-2-1:** three copies of the data, on two kinds of storage, one away from the server.

## Layer 1 — nightly backups, tested every night

`deploy/backup/nerp-backup.sh` writes a dated folder:

- `database.dump`: `pg_dump --format=custom` (compressed, can restore single tables), with a
  SHA-256 checksum, the list of applied migrations, and table sizes;
- `uploads/`: the upload folder, copied with rsync. Unchanged files are hard links to the
  previous night's copy, so every folder is a complete backup but only new files use space;
- `manifest.txt`: when, from which host, how big, which migration.

Backups older than 14 days are removed (`--keep-days`).

`deploy/backup/nerp-verify-backup.sh` then **restores the newest backup** into a temporary
database next to the live one, checks the checksum, that every table and migration is there
and that it has people in it, compares the row counts with the live database, and drops the
copy. It fails (and so does the systemd unit) if anything is wrong.

Install on the server (paths as in `deploy/systemd`, adjust them):

```sh
sudo install -d -o nerp -g nerp -m 700 /var/backups/nerp
sudo cp deploy/systemd/nerp-backup.{service,timer} /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now nerp-backup.timer          # every night at 02:30
sudo systemctl start nerp-backup.service               # once now, to see it work
journalctl -u nerp-backup.service -n 30                # "backup OK" at the end
```

The database user needs the `CREATEDB` right for the test restore
(`ALTER ROLE nerp CREATEDB;`). Get told when it fails: add `OnFailure=` to the service with a
unit that sends mail, or watch `systemctl --failed` in your monitoring.

## Layer 2 — continuous archiving and point-in-time recovery (production)

The nightly dump loses up to a day. For production, archive PostgreSQL's write-ahead log
continuously with [pgBackRest](https://pgbackrest.org), so the database can be restored to
any moment — for example to one minute before someone deleted the wrong records.

`/etc/pgbackrest/pgbackrest.conf` (one local repository and one off-site, encrypted):

```ini
[global]
repo1-path=/var/lib/pgbackrest
repo1-retention-full=2
repo2-type=s3
repo2-s3-bucket=company-nerp-backups
repo2-s3-endpoint=s3.eu-central-1.amazonaws.com
repo2-s3-region=eu-central-1
repo2-path=/nerp
repo2-retention-full=4
repo2-cipher-type=aes-256-cbc
repo2-cipher-pass=<a long random passphrase, kept in the password manager>
start-fast=y
compress-type=zst

[nerp]
# The data folder: /var/lib/postgresql/18/main on Debian/Ubuntu, /var/lib/postgres/data on Arch
pg1-path=/var/lib/postgresql/18/main
```

(Put the S3 access keys in `repo2-s3-key` / `repo2-s3-key-secret`, readable only by
`postgres`.) In `postgresql.conf`:

```ini
wal_level = replica
archive_mode = on
archive_command = 'pgbackrest --stanza=nerp archive-push %p'
archive_timeout = 300          # archive at least every 5 minutes, even when quiet
```

Then, as `postgres`:

```sh
pgbackrest --stanza=nerp stanza-create
pgbackrest --stanza=nerp check
pgbackrest --stanza=nerp --type=full backup     # weekly (cron or a timer): full
pgbackrest --stanza=nerp --type=diff backup     # daily: differential
pgbackrest info                                 # what there is to restore
```

Keep the nightly `nerp-backup.sh` too: it is independent of pgBackRest, easy to restore
anywhere, and it covers the uploaded files.

## Layer 3 — copies away from the server

Copy `/var/backups/nerp` off the machine every night, encrypted, for example with
[restic](https://restic.net) to object storage or another site:

```sh
restic -r s3:s3.eu-central-1.amazonaws.com/company-nerp-files init
restic -r s3:… backup /var/backups/nerp --tag nerp
restic -r s3:… forget --keep-daily 14 --keep-weekly 8 --keep-monthly 12 --prune
```

restic encrypts on the server before uploading; keep its password in the password manager.

## Restoring

**Look up or rescue something without touching the live system** (a deleted record, an old
version): restore into a copy next to it, then copy what you need.

```sh
psql "$ADMIN_URL" -c 'CREATE DATABASE nerp_copy'
deploy/backup/nerp-restore.sh /var/backups/nerp/2026-11-10T023000Z \
  --target-db postgres://nerp:…@127.0.0.1:5432/nerp_copy --uploads-to /tmp/nerp-files-copy
```

**Put the whole system back to a nightly backup** (this replaces the live data):

```sh
sudo systemctl stop nerp
sudo -u nerp deploy/backup/nerp-restore.sh /var/backups/nerp/2026-11-10T023000Z --yes
sudo systemctl start nerp
```

**Back to a moment in time** (pgBackRest):

```sh
sudo systemctl stop nerp postgresql
sudo -u postgres pgbackrest --stanza=nerp --delta --type=time \
  --target="2026-11-10 14:59:00+05:30" --target-action=promote restore
sudo systemctl start postgresql nerp
```

Files uploaded after the nightly snapshot are not in it; restore the folder from the night
before the moment you go back to, and expect files newer than that to be missing.

**A new server after losing the old one:** set it up as in `SETUP.md` (same NERP version),
put the saved `backend/.env` back, create the empty database, restore with `nerp-restore.sh
… --yes` (or pgBackRest for the latest state), restore the upload folder, start NERP. Check
that people can sign in and that a mailbox syncs (proof the encryption key is right).

## Restore drill

Once a month, restore the newest off-site copy on another machine (or into a copy database)
and open a few records and files. Write down how long it took. The nightly verification
proves the backup files are good; the drill proves that people know how to use them.
