# Why NERP exists

NERP is a business suite (CRM, tasks and projects, HR, calendar, messenger, drive,
accounting, help desk, booking, websites and shop, and the rest) for one company per
installation. There are already many such systems. This note explains why we built another
one, and why it is built the way it is.

## An ERP shouldn't cost an arm and a leg

The suites businesses run on are usually rented per user, per month, forever. The price
grows with every person you hire and every module you need, the features you rely on sit
behind higher tiers, and the bill keeps coming whether or not anything changed. For a small
or mid-sized company that adds up to real money every year, with nothing to show for it
except the right to keep using its own records.

NERP has no licence fee and no per-user price. You can run it for ten people or for ten
thousand at the cost of your own hardware.

## Your data is yours

A company's records — its customers, its employees, its contracts, its books — belong to the
company, not to whoever hosts the software. With NERP:

- **You choose where it lives.** Your own server, a machine in your office, a data centre
  in your country, or any cloud you trust. NERP runs on bare metal; it doesn't call home and
  needs no outside service to work.
- **You can take all of it with you at any time.** One click exports every table as CSV and
  every stored file under its own name, with a manifest of checksums (System → Backups), in
  formats any other tool can read.
- **You can prove it hasn't been tampered with.** The audit trail is hash-chained and signed
  with a key that isn't in the database.
- **You control who sees what and how long it is kept**: field-level visibility in the CRM,
  encrypted file storage, personal data requests answered from one page, and retention
  rules per kind of data.

Nobody can raise your price, change your terms, lock you out of your records or decide where
they are stored but you.

## Truly open source: GNU GPL v3

NERP is licensed under the [GNU General Public License, version 3](LICENSE). That means you
may use it for any purpose, read and study every line, change it to fit your business, and
share it — your changes included — under the same licence. Nobody, including us, can take
those freedoms away from the code once it is released, and nobody can turn a copy of it into
a closed product. There is no "community edition" with the useful parts held back: every
module is in this repository.

## Support from Nalanda Labs

Open source doesn't mean on your own. Nalanda Labs, who build NERP, help companies set it
up, keep it running, upgrade it, back it up and adapt it, for a modest fee — a fraction of
what a hosted suite charges per seat. You pay for help when you want it, not for permission
to use your own system, and you are free to do it yourself or hire anyone else instead.

## Why this technology

The stack was chosen for a system that a company runs for many years on its own machines,
with few people to look after it.

- **Rust (Axum, sqlx, Tokio) for the server.** One compiled binary, no runtime to install
  or patch, low memory use and steady performance on modest hardware. The compiler rules out
  whole classes of crashes and memory bugs, and sqlx checks database queries against their
  types. Database migrations are compiled into the binary, so an upgrade is replacing one
  file and restarting.
- **PostgreSQL, and only PostgreSQL.** One mature, free database for everything:
  transactions across modules (an invoice, its stock movement and its ledger entry commit
  together), full-text and similarity search, JSON, point-in-time recovery through streamed
  WAL, and replication. Keeping every module in one database is what makes the cross-module
  features possible — a client's whole history, profit per client including support time,
  a promise checker that weighs stock against people's calendars. NERP deliberately supports
  no second database engine: one well-understood system is easier to back up, restore and
  trust.
- **Redis only as a cache.** Sessions and everything that matters live in PostgreSQL;
  Redis holds rate limits and short-lived caches, so losing it loses nothing.
- **SvelteKit and Svelte 5 for the web app.** Small, fast pages that work on ordinary
  laptops and phones, compiled ahead of time and served by NERP itself — no separate web
  server is needed.
- **Built-in tools instead of outside services.** Documents are typeset with Typst inside
  the server; office files open in Collabora (an official, self-hosted image) only if you
  want in-browser editing; video calls use LiveKit on your own machine. Mail goes through
  your own mail server. Nothing depends on a third-party cloud.
- **Tested like it matters.** Every feature ships with API and browser tests against a real
  server and database, and a security suite of tens of thousands of checks runs against
  every route.

The result is a system you can install on one machine, understand, back up and restore, and
keep for as long as your business needs it — without paying rent on your own data.
