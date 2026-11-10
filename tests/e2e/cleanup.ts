// Every suite ends by removing what it created: all its names, test employees and test
// emails carry the run's stamp, so nothing has to be tracked one by one (see
// ../db/cleanup.ts for the database side).
import './output.ts';
import { execSync } from 'node:child_process';
import { cleanup } from '../db/cleanup.ts';

/** The local test mail accounts (Dovecot) the email suites write to. */
export const TEST_MAILBOXES = ['sales@nerp.test', 'support@nerp.test'];
const FOLDERS = ['INBOX', 'Sent'];

/** Deletes the messages whose subject contains `text` (all messages with `text` = null). */
export function clearTestMail(text) {
  for (const user of TEST_MAILBOXES) {
    for (const folder of FOLDERS) {
      const imap = (cmd) => execSync(`curl -s --max-time 10 "imap://127.0.0.1/${folder}" -u ${user}:nerptest -X '${cmd}'`, { stdio: ['ignore', 'pipe', 'ignore'] }).toString();
      try {
        const found = imap(text ? `UID SEARCH SUBJECT "${text}"` : 'UID SEARCH ALL').match(/\* SEARCH([\d ]*)/)?.[1].trim();
        if (!found) continue;
        imap(`UID STORE ${found.split(/\s+/).join(',')} +FLAGS (\\Deleted)`);
        imap('EXPUNGE');
      } catch {
        // No local mail server (or no such folder): nothing to clear.
      }
    }
  }
}

export async function cleanupRun(stamp) {
  // Mail first: a message left in a test inbox comes back as a lead the next time a suite
  // connects that mailbox.
  clearTestMail(stamp);
  try {
    const report = await cleanup({ stamp });
    const problems = report.filter((l) => /could not|not removed/.test(l));
    if (problems.length) console.log('cleanup:', problems.map((l) => l.trim()).join('; '));
  } catch (e: any) {
    console.log('cleanup failed:', e.message);
  }
}

/**
 * For the Gmail / Microsoft 365 suites: stops (as skipped, before creating anything) unless
 * the server was started with the mock OAuth provider configured (see README).
 */
export async function requireOAuthMock() {
  const B = 'http://localhost:8080/api';
  const login = await (await fetch(B + '/auth/login', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ email: 'admin@acme.test', password: 'supersecret1' }) })).json();
  const p = await (await fetch(B + '/crm/mailboxes/oauth/providers', { headers: { authorization: `Bearer ${login.access_token}` } })).json();
  if (!p.google || !p.microsoft) {
    console.log('skipped: start the server with the mock OAuth settings from README.md to run this suite');
    process.exit(0);
  }
}
