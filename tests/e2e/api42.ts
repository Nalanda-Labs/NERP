// LDAP with hostile input (a throwaway OpenLDAP server): LDAP filter injection through the
// email address or password, special characters and case, duplicate directory entries,
// several first sign-ins at once, a broken service account, deactivated directory accounts.
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { dbUrl } from '../db/cleanup.ts';
import { BASE_DN, LDAP_PORT, SERVICE_DN, SERVICE_PASSWORD, directorySkipReason, ldapReady, startLdap, type Person } from './ldap-server.ts';
import pg from 'pg';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 700)); };
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const skip = await directorySkipReason(admin, 'http://localhost:8080');
if (skip || !(await admin.get('/directory')).data.ldap_available) {
  console.log(`skipped: ${skip ?? 'start the server with LDAP_ENABLED=true'}`);
  process.exit(0);
}
const email = (n: string) => `${n}.${stamp}@acme.test`;
const weird = 'p@ss(w)*\\rd"é ✓ );(uid=*';
const people: Person[] = [
  { uid: `alice${stamp}`, given: 'Alice', sn: 'Hostile', email: email('alice'), password: 'alice-pass-1' },
  { uid: `ben${stamp}`, given: 'Ben', sn: 'Hostile', email: `Ben.${stamp}@ACME.test`, password: 'ben-pass-1' },
  { uid: `carl${stamp}`, given: 'Carl', sn: 'Hostile', email: email('carl'), password: weird },
  { uid: `dup1${stamp}`, given: 'Dup', sn: 'One', email: email('dup'), password: 'dup-pass-1' },
  { uid: `dup2${stamp}`, given: 'Dup', sn: 'Two', email: email('dup'), password: 'dup-pass-2' },
  { uid: `eve${stamp}`, given: 'Eve', sn: 'Hostile', email: email('eve'), password: 'eve-pass-1' },
  { uid: `fay${stamp}`, given: 'Fay', sn: 'Hostile', email: email('fay'), password: 'fay-pass-1' },
];
const stop = startLdap(`nerp-e2e-ldap-${stamp}`, people);
const cfg = {
  enabled: true, url: `ldap://127.0.0.1:${LDAP_PORT}`, bind_dn: SERVICE_DN, bind_password: SERVICE_PASSWORD, base_dn: BASE_DN,
  user_filter: '(objectClass=inetOrgPerson)', attr_id: 'uid', attr_email: 'mail', attr_first_name: 'givenName', attr_last_name: 'sn', auto_sync: false,
};
const login = async (e: string, password: string) => { const c = new Client(); const r = await c.post('/auth/login', { email: e, password }); return { status: r.status, data: r.data }; };
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
const accounts = async (like: string) => (await db.query(`SELECT email, auth_source FROM users WHERE email LIKE $1`, [like])).rows;

try {
  await ldapReady();
  ok('set up', (await admin.put('/directory/ldap', cfg)).status === 200);

  // ---- injection ----
  const injections = [
    `*@acme.test`, `*`, `alice.${stamp}@acme.test)(uid=*`, `*)(mail=*`, `alice.${stamp}@acme.test*`, `alice*@acme.test`,
    `\\2a@acme.test`, `alice.${stamp}@acme.test\u0000`, `(|(mail=*))@acme.test`,
  ];
  const res: number[] = [];
  for (const e of injections) res.push((await login(e, 'alice-pass-1')).status);
  ok('filter characters in the email match nobody', res.every((s) => s === 401 || s === 400), res);
  ok('and create no accounts', (await accounts('%*%')).length === 0 && (await accounts('%(%')).length === 0);
  ok('a wildcard password is just wrong', (await login(email('alice'), '*')).status === 401);
  ok('so is a filter in the password', (await login(email('alice'), `alice-pass-1)(uid=*`)).status === 401);
  ok('an empty or blank password', (await login(email('alice'), '')).status !== 200 && (await login(email('alice'), '   ')).status === 401);
  ok('a huge password', (await login(email('alice'), 'x'.repeat(100_000))).status >= 400);
  ok('the right one still works', (await login(email('alice'), 'alice-pass-1')).status === 200);

  // ---- characters and case ----
  ok('a password full of special characters', (await login(email('carl'), weird)).status === 200);
  ok('the email in any case', (await login(email('alice').toUpperCase(), 'alice-pass-1')).data.user?.email === email('alice'));
  const ben = await login(`ben.${stamp}@acme.test`, 'ben-pass-1');
  ok('a directory email in capitals', ben.status === 200 && ben.data.user.email === `ben.${stamp}@acme.test`, ben.data);

  // ---- duplicates ----
  const dup = await login(email('dup'), 'dup-pass-1');
  ok('two directory entries with one email: refused, explained', dup.status >= 400 && dup.status < 500 && /several directory entries/.test(JSON.stringify(dup.data)), dup);
  ok('and no account', (await accounts(email('dup'))).length === 0);

  // ---- several first sign-ins at once ----
  const burst = await Promise.all(Array.from({ length: 6 }, () => login(email('eve'), 'eve-pass-1')));
  ok('first sign-ins at once: all get in', burst.every((r) => r.status === 200), burst.map((r) => r.status));
  ok('with one account between them', new Set(burst.map((r) => r.data.user?.id)).size === 1 && (await accounts(email('eve'))).length === 1);

  // ---- sync ----
  const sync = (await admin.post('/directory/ldap/sync')).data;
  ok('sync skips the duplicate email', sync.skipped >= 1 && (await accounts(email('dup'))).length === 0, sync);
  ok('and stores emails in lower case', (await accounts(`ben.${stamp}@acme.test`)).length === 1 && (await accounts(`Ben.%`)).length === 0);
  const again = await Promise.all([admin.post('/directory/ldap/sync'), admin.post('/directory/ldap/sync')]);
  ok('two syncs at once: one runs, the other is told', again.map((r) => r.status).sort().join() === '200,409', again.map((r) => r.status));

  // ---- deactivated directory accounts ----
  const fay = await login(email('fay'), 'fay-pass-1');
  await admin.patch(`/users/${fay.data.user.id}`, { is_active: false });
  ok('deactivated in NERP: the directory password is not enough', (await login(email('fay'), 'fay-pass-1')).status === 401);

  // ---- a broken service account ----
  await admin.put('/directory/ldap', { ...cfg, bind_password: 'wrong' });
  const broken = await login(email('alice'), 'alice-pass-1');
  ok('a broken service account: directory people are told it can\'t be reached', broken.status === 400 && /cannot be reached/.test(JSON.stringify(broken.data)), broken);
  ok('local accounts still sign in', (await login('admin@acme.test', 'supersecret1')).status === 200);
  const s2 = (await admin.post('/directory/ldap/sync')).data;
  ok('a sync reports it and changes nothing', /refused the service account/.test(s2.error ?? ''), s2);
  ok('no one deactivated', (await db.query(`SELECT count(*)::int AS n FROM users WHERE email LIKE $1 AND NOT is_active`, [`%.${stamp}@acme.test`])).rows[0].n === 1);
} finally {
  await admin.put('/directory/ldap', { enabled: false, bind_password: '' });
  stop();
  await db.end();
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
