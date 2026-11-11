# End-to-end tests

Written in TypeScript and run directly by Node (24+; it strips the types, no build step).
`npm run check` in `tests/` type-checks them; shared helpers live in `lib.ts`.

Scripts that exercise a running NERP server the way people and other systems use it:

- `apiN.ts`: the HTTP API (permissions, validation, automation, documents, email, …);
- `uiN.ts`: the web app in a headless Chromium (Playwright).

Each run stamps everything it creates (record names, test employees' addresses) with a
random stamp and, at the end, removes everything carrying that stamp (`cleanup.ts`, built
on `../db/cleanup.ts`). Leftovers from interrupted runs: `node ../db/cleanup-test-data.ts`
(shows what it would remove; add `--yes` to remove it).

## Before running

- The backend running on `http://localhost:8080`, serving the built frontend
  (`STATIC_DIR=../frontend/build`), with `CRM_WEBHOOK_ALLOW_PRIVATE=true` (the automation
  tests call a webhook receiver on localhost) and `MAIL_ALLOW_PRIVATE=true` (the mail suites
  use Dovecot and Mailpit on localhost).
- The admin account `admin@acme.test` / `supersecret1`.
- For the email suites: Dovecot on 127.0.0.1:143 (users `sales@nerp.test` and
  `support@nerp.test`, password `nerptest`) and Mailpit (SMTP 1025, web/API 8026).
- For the document suites: `lualatex`, `soffice` and `pdftotext` installed.
- Some suites read the database directly with `psql` (`DATABASE_URL` from the backend's
  `.env`).
- For `api11` and `ui11` (Gmail / Microsoft 365 sign-in): the tests start a mock OAuth
  provider on 127.0.0.1:8099; start the server with
  `GOOGLE_CLIENT_ID=g GOOGLE_CLIENT_SECRET=shh GOOGLE_OAUTH_AUTH_URL=http://127.0.0.1:8099/google/authorize GOOGLE_OAUTH_TOKEN_URL=http://127.0.0.1:8099/google/token`
  (and the same with `MICROSOFT_…` and `/microsoft/…`). Without them, skip those two.
- For `api60` (calendar sync): the Google and Microsoft parts use the same mock OAuth provider
  on 127.0.0.1:8099 (which also mocks Google's CalDAV and Microsoft Graph); start the server
  with the `GOOGLE_…`/`MICROSOFT_…` settings above plus
  `GOOGLE_CALDAV_URL=http://127.0.0.1:8099/google/caldav/v2 MICROSOFT_GRAPH_URL=http://127.0.0.1:8099/microsoft/graph/v1.0`.
  Without them those parts are skipped; the CalDAV-account part syncs with NERP's own CalDAV
  server and needs `CRM_WEBHOOK_ALLOW_PRIVATE=true` (in the full-test environment).
- For `api63` and `ui57` (office editing): start the server with
  `COLLABORA_URL=http://127.0.0.1:9988`; the suites run a stand-in for Collabora Online there
  (its discovery, and for `ui57` its editor page). Without it the editing parts are skipped;
  the LibreOffice previews always run (they need `soffice`).
- For `ui67` (editing with the real Collabora Online): run `tools/collabora-up.sh` (the
  official `collabora/code` image in podman, on 127.0.0.1:9980, sharing the host network so it
  reaches NERP's WOPI endpoints at http://localhost:8080) and start the server with
  `COLLABORA_URL=http://127.0.0.1:9980 tools/restart-server.sh`. It edits a text document,
  an xlsx workbook and a pptx deck (made and read back with `soffice`), saving with Ctrl+S
  and on leaving; two people edit one file at once; a viewer cannot save.
  With the real Collabora connected, `api63` and `ui57` skip their stand-in editing checks.
  Stop it with `podman rm -f nerp-collabora`.
- `api59` drives NERP's CalDAV server with the `tsdav` client (installed with `npm install`).
- For `api36`, `ui37` and `api42` (directory and sign-on): OpenLDAP installed (`slapd` and `slapadd`,
  with the schemas in `/etc/openldap/schema` or, on Debian/Ubuntu, `/etc/ldap/schema`). The suites run their own throwaway `slapd` on
  127.0.0.1:3893 (configuration and database in the output folder, removed afterwards) and a
  mock OpenID Connect provider on 9477/9478. Start the server with
  `PUBLIC_URL=http://localhost:8080` so the sign-on callback comes back to it, and with
  `LDAP_ENABLED=true` for the LDAP checks (without it they only check that LDAP is refused).
  Otherwise they report themselves skipped.
- For `ui40` (video calls through TURN): coturn installed (`turnserver`), and the server
  started with
  `WEBRTC_ICE_SERVERS='[{"urls":"turn:127.0.0.1:3478?transport=udp","username":"nerp-test","credential":"nerp-test-turn"}]'`.
  The suite runs its own throwaway `turnserver` on 127.0.0.1:3478 and lets the browsers use
  relayed connections only. Otherwise it reports itself skipped.
- For `api39`, `api43` and `ui41` (calls through LiveKit, and recording): `livekit-server` installed,
  and the server started with `LIVEKIT_URL=ws://127.0.0.1:7890 LIVEKIT_API_URL=http://127.0.0.1:7899
  LIVEKIT_API_KEY=nerp-test-key LIVEKIT_API_SECRET=nerp-test-secret-0123456789abcdefghijkl
  LIVEKIT_RECORDINGS_DIR=$TMPDIR/nerp-e2e/livekit-recordings` (or `/tmp/nerp-e2e/…`). The suites run their own
  throwaway `livekit-server` on 7890, and on 7899 a stand-in for LiveKit Egress's API that checks
  NERP's signed requests and writes the "recording" where Egress would; NERP's server API calls
  go through it. Real recordings need LiveKit Egress, which these suites don't run. Otherwise
  they report themselves skipped.
- `npm ci` in `tests/` (one package for the suites and the database checks), and the Playwright Chromium headless shell
  (`npx playwright install chromium-headless-shell`).

## Running

```sh
node run-all.ts            # everything (about 10 minutes)
node run-all.ts api        # API suites only
node run-all.ts ui15 api15 # chosen suites
node api18.ts              # one suite, with every check printed
```

## Helpers (`tools/`)

Not tests, but handy while working on the UI:

- `screens-core.ts`, `screens-extra.ts`: screenshots of the main screens.
- `screens-automation.ts`: sets up a sample pipeline with robots and screenshots the
  automation screens (it removes the pipeline afterwards).
- `word-pdf-download.ts`: previews a Word template and downloads the PDF.

Screenshots (of failing UI steps, `failN-*.png`, and the deliberate ones) go to `nerp-e2e` in the system's temporary folder, or `NERP_E2E_OUTPUT`; run-all prints the folder when a step failed.

Known flaky step: `ui4` "drag bar later → dependant shifts" (a Gantt drag in a headless
browser) sometimes fails and passes on a rerun.
