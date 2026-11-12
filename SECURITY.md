# Security

How NERP is protected, the findings of the November 2026 security review and what was done
about them, the risks that remain, and the penetration test suite that keeps checking.

## Reporting a vulnerability

Please report it privately through GitHub (the repository's **Security → Report a
vulnerability**), not in a public issue. Include what an attacker can do, and the steps or
a request that shows it.

## Review of November 2026

The review read the code paths attackers reach first (sign-in and sessions, permission
checks, SQL built from strings, file serving, HTML rendering, document templates, requests
the server makes to addresses users give) and then attacked the running application with
the suite in `tests/security`. Every finding below was fixed, and each has a check in that
suite (or a unit test) so it stays fixed.

| # | Severity | Finding | Fix |
|---|----------|---------|-----|
| 1 | High | **LaTeX templates could read and write any file the server can.** LuaLaTeX runs Lua (`\directlua`) whose `io.open`, `os.remove` and `os.rename` are not limited by TeX's own file restrictions. A CRM admin could read `backend/.env` (with the token-signing secret) and from there take over any account. Shell commands were already off. | Adding, changing or previewing LaTeX code needs the **Manage system** permission, the same trust as running the server. Typst and Word templates, which cannot reach files, stay with CRM admins. |
| 2 | Medium | **A Typst template could run forever.** Typst cannot be stopped once it runs; a long loop tied up a server thread and the CPU indefinitely (tested: still running after 120 s), during previews or whenever a document was generated with that template. | Templates people write render in a child process (`nerp render-typst`) that is killed at the document timeout and limited to 4 GB of memory and 10 minutes of CPU. NERP's own fixed templates still render in-process. |
| 3 | Medium | **A Word template "zip bomb"** (a few MB that unpacks to gigabytes) would fill the server's memory. | A template may unpack to 100 MB, counted on the bytes actually read, not the sizes the archive claims. |
| 4 | Medium | **No site-wide security headers:** no Content-Security-Policy, nothing against being framed (clickjacking), `nosniff` only on some responses. | Every response gets a CSP (scripts only from NERP, plus the app's one inline bootstrap allowed by its hash), framing only by NERP itself (web forms, the chat widget, booking and help pages may be embedded anywhere), `nosniff`, a referrer policy, a permissions policy, COOP, and HSTS behind HTTPS. |
| 5 | Medium | **A stolen refresh token went unnoticed.** The token was rotated on use, but if an attacker used a stolen copy first, they kept a working session and the real person was just signed out. | Exchanged tokens are remembered; one presented again (after a 30-second grace for two tabs refreshing at once) ends the whole session for every holder, is written to the audit log and is reported to the person. |
| 6 | Medium | **Any employee could make the server connect to internal machines** by "testing" a mailbox with any host and port, and read their replies in the error message (network probing). | Mail servers on internal addresses are refused unless `MAIL_ALLOW_PRIVATE=true` (for a company mail server); checked when testing, saving, syncing and sending. |
| 7 | Low | **Password spraying** (one password tried on many accounts) was not slowed; the limit was per account only. | Besides 10 failures per account, 100 failed sign-ins per address in 15 minutes. |
| 8 | Low | **The sign-in redirect check** let `?next=/\evil.example` through (browsers read `/\` as `//`). SvelteKit's refusal to navigate away saved it. | The check itself refuses `//` and `/\`. |
| 9 | Low | **Unknown API paths answered with the app's HTML page and 200,** which looks like success to API clients. | A JSON 404. |

## Second round, 11 November 2026

After guests, public links, BI, Collabora editing, calendar apps, the shop and bookings were
added, a second round attacked those with five new suites (`pen6` to `pen10`).

| # | Severity | Finding | Fix |
|---|----------|---------|-----|
| 10 | Medium | **Guessing two-factor codes had no real limit.** Each sign-in allowed 5 wrong codes, but someone who knew the password could simply sign in again for 5 more (60 wrong codes in a row were accepted, then the right one worked). | Wrong codes are also counted per account: after 10 in 15 minutes the second step waits. Recovery codes (long and random) still work, so the real person isn't locked out by the guesser. |
| 11 | Medium | **Two bookings at the same moment could both take the last place** of a room or person (6 bookings of a room for one, sent at once, all went through). Checking that it was free and booking it were separate steps. | Booking, moving a booking and putting a calendar event in a room hold a lock on each resource (a Postgres advisory lock, so it holds across server instances) from the check to the booking. |
| 12 | Low | **A "once per customer" coupon could be used twice** by sending two orders at the same moment. | Taking a use checks and records it in one transaction under a lock on that coupon and customer. |
| 13 | Low | **A Collabora editor token outlived the sign-in it came from:** after signing out, or after a password change on another device, a copied token still read and saved the file for up to 10 hours. | The token names the sign-in (session) it was made for and stops working when that session ends. |

## Third round, 11 November 2026: every endpoint, signed in

The suites `pen11` to `pen19` turn the attacks into one check per endpoint (and per
credential, person, thing or payload), mostly **as signed-in people**: an ordinary employee,
a manager of another module, a colleague going after another's things, a guest from outside,
and API keys. To make sure each request reaches the code that decides (not just the check
of its shape), the sweeps complete every body from the server's own "missing field" and
"invalid type" answers before judging the result. About 35,000 checks in all, against 686
paths (939 endpoints).

| # | Severity | Finding | Fix |
|---|----------|---------|-----|
| 14 | Low | **Private API answers could be cached.** Responses carried no `Cache-Control`, so a browser's back-forward cache or a shared proxy could keep someone's private data. | Every API answer that doesn't set its own caching gets `Cache-Control: no-store`. |

### Checked and found sound

- **Tokens:** HS256 with the algorithm fixed (unsigned "alg: none" and altered tokens are
  refused); 15-minute access tokens; signed-out sessions are refused on the next request;
  a password change signs out other devices; deactivation takes effect at once.
- **Refresh cookie:** `HttpOnly`, `SameSite=Strict`, sent only to `/api/auth`; the API
  itself uses a bearer token, so cross-site requests cannot act as the user.
- **Brute force:** 10 failures lock an account for 15 minutes; two-factor codes have their own
  attempt limit; reset links are single-use, expire and are rate-limited; sign-in and reset
  answer the same for unknown accounts.
- **SQL:** values are always bound parameters; the strings assembled into SQL are fixed
  column lists and conditions, never user input (23,000 injection requests found nothing).
- **Permissions:** each module checks its permission; an employee reached none of another
  employee's private things through any of the 531 routes.
- **Requests to addresses users give** (webhooks): the name is resolved once, every
  address is checked against internal ranges, and the connection is pinned to it (no DNS
  rebinding).
- **Uploads:** only images, PDFs and media display in the browser; everything else downloads,
  always with `nosniff` and a sandboxing CSP; file names cannot climb directories or add
  headers; size limits hold.
- **HTML:** Markdown is rendered then cleaned with `ammonia`; website links are limited to
  `https://`, `/`, `#`, `mailto:` and `tel:`; email HTML is cleaned and shown in a sandboxed
  frame; CSV exports defuse spreadsheet formulas.
- **Templates:** TeX cannot read files outside its folder or run commands; Typst templates
  can only read the template and its data; LibreOffice does not follow `INCLUDETEXT` fields.
- **Guests** reached nothing outside their own chats, workgroups and profile through any of
  the 685 routes (tried with every method and with company ids).
- **Newer private things** (boards, private workgroups and their posts, dashboards, app
  passwords, sessions, drive links, absence requests) were out of a colleague's reach
  through every route with one or two ids, and missing from their lists and searches.
- **Other tokens:** Collabora tokens are bound to one file and person and re-check access
  on every call; calendar app passwords open only their owner's calendar, are not sign-in
  passwords and are rate-limited; real-time tickets work once; download links open only
  their own file; API keys reach nothing outside their scopes (path tricks included).
- **Requests the server makes** (BI datasets from URLs, calendar accounts, mailboxes) refuse
  internal addresses in every spelling tried (`127.1`, `2130706433`, `0x7f000001`, octal,
  `[::ffff:127.0.0.1]`, link-local metadata, private ranges, other schemes), tested on a
  server with the production defaults.
- **Shop:** prices come from the catalogue, never the visitor; odd quantities are refused;
  limited coupons hold under concurrent orders.
- **Join links** refuse look-alike domains, and joining with an existing person's address
  changes nothing about them. **Forged `X-Forwarded-For`** passes neither the sign-in limit
  nor the company IP rule; **forged `Host`/`X-Forwarded-Host`** doesn't select a website.
- **Emails** show names with markup as text (invitations, campaigns merged with contacts from
  public forms); a line break in a name adds no recipients. **Public forms** ignore fields
  they don't have; one live-chat visitor can't read another's conversation.

## SAML sign-on

- Responses must carry a valid signature by the identity provider's certificate from its
  metadata (xmlsec); metadata without a signing certificate is refused when it is saved, since
  the library would otherwise accept unsigned responses. Only the signed part of the document
  is read, so a forged assertion wrapped around a genuine one can't stand in for it (tested:
  it signs in as the genuine person, never the forged one). SHA-1 and DSA signatures are
  refused.
- Issuer, audience, destination, recipient, NotBefore/NotOnOrAfter (3 minutes' clock skew,
  5 minutes' issue delay), the bearer confirmation and InResponseTo (the request NERP sent,
  remembered for 10 minutes and used once) are all checked; each assertion ID is accepted once
  (an hour in Redis); IdP-initiated sign-in is refused.
- Tested against a stand-in identity provider with unsigned, wrongly signed, altered, SHA-1,
  wrapped, wrong-audience, expired, replayed, unrequested and other-request answers
  (tests/e2e/api103.ts).

## Encryption at rest

- **Uploaded files** are encrypted by NERP when `FILE_ENCRYPTION_KEY` is set: AES-256-GCM in
  64 KiB chunks, a key per file derived from the master key and a random salt, each chunk's
  nonce its number with a flag on the last one (so a changed, reordered or shortened file
  fails to read rather than serving altered content). Downloads and byte ranges decrypt only
  the chunks they need; tools that need a path (ffmpeg) get a private temporary copy that is
  removed afterwards. Files from before are encrypted by a background job (one run at a time,
  under a database lock); a key can be replaced (`FILE_ENCRYPTION_OLD_KEYS`) the same way. The
  server refuses to start if files are under a key it wasn't given, instead of serving errors.
- **Secrets in the database** (mailbox, directory, calendar and data-source passwords, OAuth
  tokens) are encrypted with `DATA_ENCRYPTION_KEY`.
- **The database itself** is not encrypted by NERP (PostgreSQL has no built-in encryption of
  its files): put PostgreSQL's data folder, the backup folder and the WAL archive on encrypted
  disks (LUKS), and encrypt copies that leave the server (restic, pgBackRest's cipher).

## Risks that remain

- **LaTeX templates are code.** People with "Manage system" can still write a template that
  reads server files. For defence in depth, run `lualatex` in an operating-system sandbox
  (bubblewrap, a container) that sees only the TeX tree, fonts and its working folder.
- **Account lockout can be used to annoy:** anyone who knows an address can lock it for 15
  minutes with wrong passwords. This is the usual trade-off against password guessing.
- **Behind a reverse proxy, set `TRUST_PROXY=true`.** Otherwise every request seems to come
  from the proxy, so the per-address sign-in limit would lock out everyone at once, and the
  audit log would show the proxy's address.
- **The CSP allows inline styles** (`style="…"` attributes are used throughout the app) and
  images from any `https://` address (pictures on websites and in Markdown).
- **Dependencies are not scanned automatically yet.** Adding `cargo audit` and `npm audit` to CI
  is the next step.

## The penetration test suite

`tests/security` attacks a running server (the same one the end-to-end suites use) and fails
on any weakness. Run it with `node tests/security/run.ts` (or `npm run security` in `tests/`);
it needs `redis-cli`, to reset the rate limits it trips on purpose, and Mailpit (for `pen6` and
`pen10`). `pen7` checks Collabora tokens when the server has `COLLABORA_URL`; `pen9` starts its
own second server on 127.0.0.1:8091 from `backend/target/debug/nerp`. `world.ts` builds the people
and private things the sweeps use, and `sweep.ts` holds the shared sweep code.

| Suite | What it tries |
|-------|---------------|
| `pen1` | Every route without signing in (all must refuse); forged, altered, unsigned, expired and signed-out tokens; password change and deactivation; a stolen refresh token used twice; brute force; password spraying; finding out which accounts exist. |
| `pen2` | An employee raising their own rights or using admin-only actions; reaching another employee's task, file, drive file, chat, private event and ticket through **every route with an id** and every method; odd input on every route (no server errors). |
| `pen3` | SQL injection (including time-based, with `pg_sleep`), template injection, null bytes, invalid Unicode and oversized values in the query parameters of every read route and the text fields of every write route. |
| `pen4` | Script injection through Markdown (knowledge base, training, websites); uploads of HTML, SVG and disguised files; path traversal and header injection in file names; size limits; the security headers; CORS from another site; a forged `Host` header in password-reset links. |
| `pen5` | LaTeX and Typst templates reading files or running commands; who may write LaTeX; a looping Typst template; a Word zip bomb. |
| `pen6` | A colleague against the newer private things (board, private workgroup and its posts, dashboard, app password, session, drive link, absence request) through every route with **one or two ids**, and in lists and searches; a guest against **every route**. |
| `pen7` | Collabora (WOPI) tokens: other files, tampering, revoked shares, view-only, signing out, password change, deactivation; calendar app passwords (CalDAV): other people's calendars, revoking, guessing; real-time tickets; download links; API key scopes; two-factor codes: replay, recovery codes, guessing over fresh sign-ins. |
| `pen8` | Shop prices and quantities sent by visitors; coupons and room bookings raced by concurrent requests; read-only board links writing; join links with look-alike domains; sign-in redirects to other sites; forged `X-Forwarded-For` against limits and the IP rule; forged `Host`. |
| `pen9` | Starts a second server with the production defaults: SSRF through BI URL datasets, calendar accounts and mailboxes in every address spelling; cookie flags; a room raced by calendar events; setting one's own roles, email or guest status. |
| `pen10` | Markup and line breaks in names reaching emails (invitations, campaigns via public forms); fields a public form doesn't have; one live-chat visitor reading another's conversation. |
| `pen11` | Every private endpoint × 11 kinds of missing or broken credentials (no token, garbage, "alg: none" as the admin, altered payload, wrong signature, signed-out session, deactivated person, token in the address, refresh cookie alone, Basic auth, API-key-shaped string): about 9,300 checks. |
| `pen12` | Signed in as an employee, every endpoint with odd bodies and odd ids: no server error, no internal detail, an answer within 5 s, and `no-store`/`nosniff` on private answers. |
| `pen13` | Every permission-gated endpoint (read from the source) by an employee without the permission and by a manager of another module, bodies completed until the handler answers; admin areas with plausible bodies. |
| `pen14` | A colleague against 26 private things of another person (tasks, files, drive, chats and messages, posts and comments, private workgroups, Scrum, events, meetings, tickets, boards and links, dashboards, knowledge base, lists, app passwords, sessions, leave, work reports) through every endpoint with one or two ids: about 14,000 checks, then everything checked unchanged. |
| `pen15` | A signed-in guest against every endpoint, with no ids and with each of the company's things: nothing outside the guest's areas, nothing of the company shown. |
| `pen16` | CRM-read, tasks and users-read API keys against every endpoint; keys inside their area act as their owner; revoking, losing the permission and deactivation stop them. |
| `pen17` | Signed in, 15 injection payloads (SQL error- and time-based, templates, shell, traversal, header line breaks, null bytes, NoSQL, markup) in every text field and query parameter of every endpoint: no error, no delay, no live markup. |
| `pen18` | Mass assignment on every create and change endpoint: owner, creator and author fields, moving one's things into someone else's workgroup, folder, chat, space, list, board, dashboard or calendar, and privilege flags. |
| `pen19` | Business rules, signed in: approving one's own leave or someone's expense, editing or deleting colleagues' messages, posts, comments and tasks, voting twice, promoting oneself in a group or chat, widening one's own drive share, changing an event one is invited to, reading a help desk's internal notes or a test's answers, closing others' meetings, taking another email address. |

The suites read the list of routes from the backend's source, so new routes are attacked
without changing the tests.
