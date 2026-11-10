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
it needs `redis-cli`, to reset the rate limits it trips on purpose.

| Suite | What it tries |
|-------|---------------|
| `pen1` | Every route without signing in (all must refuse); forged, altered, unsigned, expired and signed-out tokens; password change and deactivation; a stolen refresh token used twice; brute force; password spraying; finding out which accounts exist. |
| `pen2` | An employee raising their own rights or using admin-only actions; reaching another employee's task, file, drive file, chat, private event and ticket through **every route with an id** and every method; odd input on every route (no server errors). |
| `pen3` | SQL injection (including time-based, with `pg_sleep`), template injection, null bytes, invalid Unicode and oversized values in the query parameters of every read route and the text fields of every write route. |
| `pen4` | Script injection through Markdown (knowledge base, training, websites); uploads of HTML, SVG and disguised files; path traversal and header injection in file names; size limits; the security headers; CORS from another site; a forged `Host` header in password-reset links. |
| `pen5` | LaTeX and Typst templates reading files or running commands; who may write LaTeX; a looping Typst template; a Word zip bomb. |

The suites read the list of routes from the backend's source, so new routes are attacked
without changing the tests.
