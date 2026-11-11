# Setting up NERP

From an empty machine to a running NERP: first for development, then for production on a
Linux server behind HTTPS. Settings are explained in the README's
[Configuration](README.md#configuration) table; backups in [BACKUP.md](BACKUP.md); security in
[SECURITY.md](SECURITY.md).

## What NERP needs

| | Needed for | Notes |
|---|---|---|
| **PostgreSQL 18** | everything | with the `pgcrypto` and `pg_trgm` extensions (both ship with PostgreSQL) |
| **Redis or Valkey** | everything | sessions, rate limits, live updates between instances |
| **Rust** (stable, 1.85+) | building the server | |
| **Node 24** | building the web app, running the tests | |
| SMTP server | sending email | any provider; Mailpit for development |
| Noto fonts | documents in every script | `fonts-noto-core fonts-noto-cjk` / `noto-fonts noto-fonts-cjk` |
| TeX Live with LuaLaTeX | LaTeX document templates (optional) | `texlive-luatex texlive-latex-extra` / `texlive-luatex texlive-latexextra` |
| LibreOffice | Word document templates (optional) | `libreoffice-writer-nogui` / `libreoffice-fresh` |
| coturn, LiveKit | video calls across networks, recording (optional) | see the README's video calls section |
| PostgreSQL 18 client and server programs | backups and point-in-time restores, where backups are kept | `postgresql-client-18`, `postgresql-18` (see step 1 when the database is on another machine) |

## Development

```sh
git clone git@github.com:Nalanda-Labs/NERP.git && cd NERP

# PostgreSQL 18 in a container (or a local install), and Valkey/Redis
podman run -d --name nerp-postgres -p 127.0.0.1:5432:5432 \
  -e POSTGRES_USER=nerp -e POSTGRES_PASSWORD=devpass -e POSTGRES_DB=nerp \
  -v nerp-pgdata:/var/lib/postgresql docker.io/library/postgres:18.6-alpine
podman run -d --name nerp-valkey -p 127.0.0.1:6379:6379 docker.io/valkey/valkey:8
# Mailpit catches outgoing email: SMTP on 1025, web on http://localhost:8025
podman run -d --name mailpit -p 127.0.0.1:1025:1025 -p 127.0.0.1:8025:8025 docker.io/axllent/mailpit

# The server
cd backend
cp .env.example .env
#   set DATABASE_URL=postgres://nerp:devpass@127.0.0.1:5432/nerp
#   set JWT_SECRET to 32+ random characters:  openssl rand -base64 48
#   set COOKIE_SECURE=false (plain http://localhost)
cargo run                       # http://127.0.0.1:8080 — creates the tables on first start

# The web app, in another terminal
cd frontend
npm install
npm run dev                     # http://localhost:5173 (passes /api to the server)
```

Open http://localhost:5173/setup to create the company and the first administrator.
(Docker works the same: replace `podman` with `docker`. After a reboot,
`podman start nerp-postgres nerp-valkey mailpit`.)

To run the end-to-end and security tests, the server needs the extra settings and local
services listed in [tests/e2e/README.md](tests/e2e/README.md).

## Production

The steps for Debian/Ubuntu; Arch package names are noted where they differ.

### 1. Packages

```sh
# PostgreSQL 18 from the PostgreSQL project's repository (Ubuntu 24.04 ships 16)
sudo apt install -y postgresql-common
sudo /usr/share/postgresql-common/pgdg/apt.postgresql.org.sh
sudo apt install -y postgresql-18 redis-server build-essential pkg-config rsync git \
  fonts-noto-core fonts-noto-cjk          # valkey-server instead of redis-server where packaged
# Optional document engines
sudo apt install -y texlive-luatex texlive-latex-recommended texlive-latex-extra fonts-texgyre \
  libreoffice-writer-nogui poppler-utils
# Node 24: from nodejs.org or your distribution (Rust is installed for the nerp user below)
```

(Arch: `postgresql valkey noto-fonts noto-fonts-cjk texlive-luatex texlive-latexextra
libreoffice-fresh nodejs`.)

**When PostgreSQL runs on another machine** (the usual case beyond a small office), install
`postgresql-18` there as above, and on the NERP machine only what the backups need, from the
same PostgreSQL repository:

```sh
# Client programs: pg_dump, pg_basebackup and pg_receivewal for backups (same major version)
sudo apt install -y postgresql-client-18
# Server programs, for point-in-time restores only (replaying WAL takes PostgreSQL itself).
# Don't let the package start a database of its own on this machine:
echo 'create_main_cluster = false' | sudo tee -a /etc/postgresql-common/createcluster.conf
sudo apt install -y postgresql-18
```

### 2. Database and Redis

```sh
sudo -u postgres psql <<'SQL'
CREATE ROLE nerp LOGIN PASSWORD 'a-long-random-password' CREATEDB;
CREATE DATABASE nerp OWNER nerp;
SQL
```

`CREATEDB` lets the nightly backup check restore into a temporary database. Keep PostgreSQL and
Valkey listening on localhost only (the defaults). If Valkey has a password (`requirepass`),
put it in `REDIS_URL` as `redis://:password@127.0.0.1:6379`.

For point-in-time restores (step 8) the `nerp` role also streams the database's change log:

```sh
sudo -u postgres psql -c 'ALTER ROLE nerp REPLICATION;'
```

**PostgreSQL on another machine** (say `10.0.0.4`, with NERP and the backups on `10.0.0.5`):
on the database server, listen on the private network only and let the NERP machine in, both
for normal connections and for streaming WAL:

```ini
# /etc/postgresql/18/main/postgresql.conf
listen_addresses = 'localhost,10.0.0.4'
ssl = on                                  # the Debian package sets up a certificate
max_slot_wal_keep_size = 20GB             # never fill the disk if receiving WAL stops for long
# (wal_level = replica and max_wal_senders = 10 are the defaults and are enough)

# /etc/postgresql/18/main/pg_hba.conf
hostssl  nerp         nerp  10.0.0.5/32  scram-sha-256
hostssl  postgres     nerp  10.0.0.5/32  scram-sha-256   # the backup check creates a scratch database
hostssl  replication  nerp  10.0.0.5/32  scram-sha-256   # pg_basebackup and pg_receivewal
```

```sh
sudo systemctl restart postgresql
sudo ufw allow from 10.0.0.5 to any port 5432 proto tcp
```

Then use `DATABASE_URL=postgres://nerp:…@10.0.0.4:5432/nerp?sslmode=require` in step 4.
Nothing NERP-specific is installed on the database server.

### 3. NERP itself

```sh
sudo useradd --system --home /opt/nerp --shell /usr/sbin/nologin nerp
sudo git clone https://github.com/Nalanda-Labs/NERP.git /opt/nerp
sudo chown -R nerp:nerp /opt/nerp
cd /opt/nerp
# Rust for the nerp user (into /opt/nerp/.cargo)
sudo -u nerp bash -c "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal"
sudo -u nerp bash -c 'cd frontend && npm ci && npm run build'                     # -> frontend/build
sudo -u nerp bash -lc 'cd backend && ~/.cargo/bin/cargo build --release'          # -> backend/target/release/nerp
```

### 4. Settings

```sh
sudo -u nerp cp backend/.env.example backend/.env
sudo chmod 600 backend/.env
openssl rand -base64 48      # run three times: JWT_SECRET, DATA_ENCRYPTION_KEY, FILE_ENCRYPTION_KEY
```

The settings that matter in production (`backend/.env`):

```ini
BIND_ADDR=127.0.0.1:8080                 # only the reverse proxy talks to NERP
DATABASE_URL=postgres://nerp:a-long-random-password@127.0.0.1:5432/nerp
REDIS_URL=redis://127.0.0.1:6379
JWT_SECRET=<random, 32+ characters>
DATA_ENCRYPTION_KEY=<random, 32+ characters>   # encrypts mailbox passwords; never change it later
FILE_ENCRYPTION_KEY=<random, 32+ characters>   # encrypts uploaded files; lose it and the files are gone
PUBLIC_URL=https://erp.example.com       # used in links in emails
STATIC_DIR=../frontend/build             # NERP serves the web app itself
COOKIE_SECURE=true                       # HTTPS only (also turns on HSTS)
TRUST_PROXY=true                         # behind the proxy: needed for real client addresses
SMTP_HOST=smtp.example.com
SMTP_PORT=587
SMTP_SECURITY=starttls
SMTP_USERNAME=…
SMTP_PASSWORD=…
MAIL_FROM="Example ERP <erp@example.com>"
UPLOAD_DIR=./data/uploads
BACKUP_DIR=/var/backups/nerp             # nightly backups and the WAL archive (step 8)
BACKUP_RESTORE=false                     # true: System → Backups may restore over the live data
```

`FILE_ENCRYPTION_KEY` is best set before the first upload; set later, existing files are
encrypted in the background (System → Storage shows the progress). To replace it later, move
the old key to `FILE_ENCRYPTION_OLD_KEYS` and set the new one; files move over by themselves.

`TRUST_PROXY=true` matters: without it every request seems to come from the proxy, and the
sign-in limit per address would lock everyone out together. Leave `CRM_WEBHOOK_ALLOW_PRIVATE`
and `MAIL_ALLOW_PRIVATE` off unless webhooks or mailboxes must reach your internal network.

**Save a copy of `backend/.env` in the company's password manager now** (see
[BACKUP.md](BACKUP.md)). Without its `FILE_ENCRYPTION_KEY` no backup of the files can be read.

### 5. Run it as a service

```sh
sudo cp deploy/systemd/nerp.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now nerp
journalctl -u nerp -f            # "database migrations are up to date", "NERP listening"
curl -s http://127.0.0.1:8080/api/health
```

The unit runs NERP as `nerp`, lets it write only its upload folder, the backup folder
(`/var/backups/nerp`, for System → Backups) and `/tmp`, and restarts it if it stops. If
`BACKUP_DIR` is elsewhere, change `ReadWritePaths` in the unit to match.

### 6. HTTPS in front

[Caddy](https://caddyserver.com) gets and renews the certificate by itself.
`/etc/caddy/Caddyfile`:

```
erp.example.com {
    encode zstd gzip
    request_body {
        max_size 60MB              # a little above MAX_UPLOAD_BYTES
    }
    reverse_proxy 127.0.0.1:8080
}
```

Or nginx (with a certificate from certbot):

```nginx
server {
    listen 443 ssl http2;
    server_name erp.example.com;
    ssl_certificate     /etc/letsencrypt/live/erp.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/erp.example.com/privkey.pem;
    client_max_body_size 60m;

    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        # live updates and calls use WebSockets
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_read_timeout 1h;
    }
}
server { listen 80; server_name erp.example.com; return 301 https://$host$request_uri; }
```

Open only ports 80 and 443 to the internet (plus coturn's ports if you run it):
`sudo ufw allow 80,443/tcp && sudo ufw enable`.

### 7. First start

Open `https://erp.example.com/setup`, create the company and the first administrator, then
invite people from Employees. Set up two-factor sign-in for administrators (Profile →
Security).

### 8. Backups

On the NERP machine (or wherever backups are kept; it needs the same `backend/.env`):

```sh
sudo install -d -o nerp -g nerp -m 700 /var/backups/nerp /var/backups/nerp/wal

# Nightly: database dump, uploaded files, a base backup for point-in-time restores, and a
# test restore of it into a scratch database.
sudo cp deploy/systemd/nerp-backup.{service,timer} /etc/systemd/system/

# All the time: the database's change log (WAL), streamed over the network into
# /var/backups/nerp/wal through a replication slot, so the database can be put back as it
# was at any moment since the oldest base backup.
sudo cp deploy/systemd/nerp-wal-receive.service /etc/systemd/system/

sudo systemctl daemon-reload
sudo systemctl enable --now nerp-wal-receive.service
journalctl -u nerp-wal-receive.service -n 20     # "receiving WAL into /var/backups/nerp/wal"
sudo systemctl enable --now nerp-backup.timer    # every night at 02:30
sudo systemctl start nerp-backup.service         # once now: makes the first base backup
journalctl -u nerp-backup.service -n 30          # "backup OK" at the end
```

Then check System → Backups: the backup is listed, and *Point-in-time restore* says the
changes are being received and since when the database can be restored. Start the receiver
before the first backup — base backups are taken only while it runs. If it stops, the page
turns red: the database server keeps the changes for it (up to `max_slot_wal_keep_size`), so
start it again soon. To give up point-in-time restores, stop the service and drop its slot
(`SELECT pg_drop_replication_slot('nerp_wal');`), or the server keeps WAL for nobody.

Copy `/var/backups/nerp` off the machine every night, encrypted, and practise a restore: both
in [BACKUP.md](BACKUP.md), which also has the restore commands.

### Updating

```sh
cd /opt/nerp
sudo systemctl start nerp-backup.service          # a fresh backup first
sudo -u nerp git pull
sudo -u nerp bash -c 'cd frontend && npm ci && npm run build'
sudo -u nerp bash -lc 'cd backend && ~/.cargo/bin/cargo build --release'
sudo systemctl restart nerp
```

Database changes apply on start: each migration runs in one transaction (a failed one changes
nothing), and when several instances start together only one migrates while the others wait.

### Several instances

Run more instances on other machines with the same `.env` (same database, Redis and
`UPLOAD_DIR` on shared storage), behind one proxy. Background jobs are shared out
automatically; set `WORKER_ENABLED=false` on instances that should only answer requests.
Run the backups and `nerp-wal-receive` on one machine only.

### Optional features

Connecting Gmail and Microsoft 365, directory sign-in (LDAP, single sign-on), video calls
through TURN or LiveKit, and the document engines are each described in their section of the
[README](README.md).
