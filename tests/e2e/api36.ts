// Directory and single sign-on. LDAP (a throwaway OpenLDAP server): settings and the
// connection test, sign-in with the directory password and accounts created on first
// sign-in, sync (new people, changed details, linking existing accounts, people who leave
// and come back, a filter that finds no one), password changes refused, an unreachable
// directory. OpenID Connect (a mock provider): the button, the round trip with PKCE and a
// nonce, matching by email and by subject, creating accounts, allowed domains, replayed and
// forged responses, deactivated people, a provider that refuses.
import { API, BASE, Client, waitFor } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { BASE_DN, LDAP_PORT, SERVICE_DN, SERVICE_PASSWORD, directorySkipReason, ldapReady, startLdap, type Person } from './ldap-server.ts';
import { startMockOidc } from './mock-oidc.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 700)); };
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const skip = await directorySkipReason(admin, BASE);
if (skip) { console.log(`skipped: ${skip}`); process.exit(0); }
const email = (n: string) => `${n}.${stamp}@acme.test`;
const login = async (e: string, password: string) => { const c = new Client(); const r = await c.post('/auth/login', { email: e, password }); return Object.assign(c, { status: r.status, data: r.data }); };

const dept = (await admin.post('/departments', { name: `Directory ${stamp}` })).data;
const people: Person[] = [
  { uid: `alice${stamp}`, given: 'Alice', sn: 'Lidap', email: email('alice'), password: 'alice-pass-1', title: 'Engineer', phone: '+1 555 0101', department: `Directory ${stamp}` },
  { uid: `bob${stamp}`, given: 'Bob', sn: 'Lidap', email: email('bob'), password: 'bob-pass-1' },
  { uid: `carol${stamp}`, given: 'Carol', sn: 'Lidap', email: email('carol'), password: 'carol-pass-1', title: 'Designer' },
  { uid: `dave${stamp}`, given: 'Dave', sn: 'Directory', email: email('dave'), password: 'dave-dir-pass' },
];
const container = `nerp-e2e-ldap-${stamp}`;
const stop = startLdap(container, people);
const oidc = await startMockOidc(9477);
const ldapCfg = {
  enabled: true, url: `ldap://127.0.0.1:${LDAP_PORT}`, bind_dn: SERVICE_DN, base_dn: BASE_DN,
  user_filter: '(objectClass=inetOrgPerson)', attr_id: 'uid', attr_email: 'mail', attr_first_name: 'givenName', attr_last_name: 'sn',
  attr_phone: 'telephoneNumber', attr_position: 'title', attr_department: 'departmentNumber', auto_sync: false,
};

const ldapOn = (await admin.get('/directory')).data.ldap_available === true;

let r: any;
try {
  ldap: {
  if (!ldapOn) {
    // The server runs without LDAP_ENABLED=true: LDAP is not offered at all.
    ok('LDAP off on this server: settings refused', (await admin.put('/directory/ldap', ldapCfg)).status === 400);
    ok('LDAP off: test and sync refused', (await admin.post('/directory/ldap/test')).status === 400 && (await admin.post('/directory/ldap/sync')).status === 400);
    ok('LDAP off: directory people cannot sign in', (await login(email('alice'), 'alice-pass-1')).status === 401);
    console.log('(LDAP checks skipped: start the server with LDAP_ENABLED=true to run them)');
    break ldap;
  }
  await ldapReady();
  // ---- LDAP settings ----
  const outsider = await (async () => { await admin.post('/users', { email: email('olga'), password: 'password123', first_name: 'Olga', last_name: 'Local' }); return login(email('olga'), 'password123'); })();
  ok('settings are for administrators', (await outsider.get('/directory')).status === 403 && (await outsider.put('/directory/ldap', ldapCfg)).status === 403);
  ok('an address is needed to turn it on', (await admin.put('/directory/ldap', { ...ldapCfg, url: '' })).status === 400);
  ok('only ldap:// or ldaps://', (await admin.put('/directory/ldap', { ...ldapCfg, url: 'http://x' })).status === 400);
  ok('the filter is checked', (await admin.put('/directory/ldap', { ...ldapCfg, user_filter: 'objectClass=person' })).status === 400);
  r = await admin.put('/directory/ldap', { ...ldapCfg, enabled: false, bind_password: 'wrong' });
  ok('saved', r.status === 200 && r.data.ldap.url === ldapCfg.url && r.data.ldap_has_password === true && !JSON.stringify(r.data).includes('wrong'), r.data);
  // (Waits while the directory starts up.)
  const refused = await waitFor(async () => { const t = await admin.post('/directory/ldap/test'); return t.data.error?.includes('refused') ? t : null; });
  ok('the test reports a wrong service password', refused?.data.ok === false, refused?.data);
  await admin.put('/directory/ldap', { ...ldapCfg, enabled: false, bind_password: SERVICE_PASSWORD });
  r = await admin.post('/directory/ldap/test');
  ok('the test finds the employees', r.data.ok === true && r.data.found === 4 && r.data.sample.some((p: any) => p.email === email('alice') && p.name === 'Alice Lidap'), r.data);
  ok('a sync needs it on', (await admin.post('/directory/ldap/sync')).status === 400);
  r = await admin.put('/directory/ldap', ldapCfg);
  ok('turned on, the password kept', r.status === 200 && r.data.ldap.enabled && r.data.ldap_has_password, r.data);

  // ---- signing in with the directory ----
  const alice = await login(email('alice'), 'alice-pass-1');
  ok('someone in the directory signs in: an account is made', alice.status === 200 && alice.data.user.email === email('alice'), alice.data);
  const aliceUser = alice.data.user;
  ok('details come from the directory', aliceUser.first_name === 'Alice' && aliceUser.last_name === 'Lidap' && aliceUser.position === 'Engineer' && aliceUser.phone === '+1 555 0101', aliceUser);
  ok('the department is matched by name', (aliceUser.departments ?? []).some((d: any) => d.id === dept.id), aliceUser.departments);
  ok('a wrong directory password fails', (await login(email('alice'), 'nope-nope')).status === 401);
  ok('an empty password fails', (await login(email('alice'), '')).status !== 200);
  ok('someone in neither fails', (await login(email('zed'), 'whatever1')).status === 401);
  ok('a second sign-in reuses the account', (await login(email('alice'), 'alice-pass-1')).data.user?.id === aliceUser.id);
  r = await alice.post('/auth/change-password', { current_password: 'alice-pass-1', new_password: 'another-pass-1' });
  ok('directory passwords are not changed here', r.status === 400 && /directory/.test(r.data.error?.message), r.data);

  // An account that existed before is linked by email and moves to the directory password.
  const daveLocal = (await admin.post('/users', { email: email('dave'), password: 'dave-local-pass', first_name: 'David', last_name: 'Local' })).data;
  ok('before the sync: the local password', (await login(email('dave'), 'dave-local-pass')).status === 200);

  // ---- sync ----
  r = await admin.post('/directory/ldap/sync');
  ok('a sync', r.status === 200 && r.data.found === 4 && r.data.created === 2 && r.data.updated === 1 && r.data.deactivated === 0, r.data);
  const users = async () => (await admin.get(`/users?q=${stamp}&status=all&limit=100`)).data;
  let list = await users();
  const find = (e: string) => (list.items ?? list.data ?? list).find((u: any) => u.email === e);
  ok('people are added', find(email('bob')) && find(email('carol'))?.position === 'Designer', list);
  ok('an existing account is linked', find(email('dave'))?.id === daveLocal.id && find(email('dave'))?.first_name === 'Dave');
  ok('linked: the local password stops working', (await login(email('dave'), 'dave-local-pass')).status === 401);
  ok('linked: the directory password works', (await login(email('dave'), 'dave-dir-pass')).status === 200);
  r = await admin.get('/directory');
  ok('the last sync is shown', r.data.ldap_synced_at && r.data.ldap_sync_result.created === 2 && r.data.accounts.ldap >= 4, r.data);
  r = await admin.post('/directory/ldap/sync');
  ok('a second sync changes nothing', r.data.created === 0 && r.data.updated === 0 && r.data.deactivated === 0, r.data);

  // Someone leaves the directory (here: filtered out) and comes back.
  const carol = await login(email('carol'), 'carol-pass-1');
  ok('carol signs in', carol.status === 200);
  await admin.put('/directory/ldap', { ...ldapCfg, user_filter: `(&(objectClass=inetOrgPerson)(!(uid=carol${stamp})))` });
  r = await admin.post('/directory/ldap/sync');
  ok('someone who left is deactivated', r.data.deactivated === 1, r.data);
  ok('their sessions end', (await carol.get('/auth/me')).status === 401);
  ok('and they cannot sign in', (await login(email('carol'), 'carol-pass-1')).status === 401);
  ok('deactivated in the list', (await users()).items.find((u: any) => u.email === email('carol'))?.is_active === false);
  await admin.put('/directory/ldap', ldapCfg);
  r = await admin.post('/directory/ldap/sync');
  ok('coming back reactivates them', r.data.reactivated === 1, r.data);
  ok('and they sign in again', (await login(email('carol'), 'carol-pass-1')).status === 200);

  // Someone an administrator turned off stays off.
  list = await users();
  await admin.patch(`/users/${find(email('bob')).id}`, { is_active: false });
  r = await admin.post('/directory/ldap/sync');
  ok('an administrator\'s deactivation is kept', r.data.reactivated === 0 && (await login(email('bob'), 'bob-pass-1')).status === 401, r.data);

  // A filter that matches no one changes nothing.
  await admin.put('/directory/ldap', { ...ldapCfg, user_filter: '(uid=nobody-at-all)' });
  r = await admin.post('/directory/ldap/sync');
  ok('an empty result changes nothing', r.status === 200 && /no employees/.test(r.data.error ?? '') && (await login(email('alice'), 'alice-pass-1')).status === 401, r.data);
  await admin.put('/directory/ldap', ldapCfg);
  ok('the filter restored', (await login(email('alice'), 'alice-pass-1')).status === 200);

  // The directory is down.
  await admin.put('/directory/ldap', { ...ldapCfg, url: 'ldap://127.0.0.1:1' });
  r = await login(email('alice'), 'alice-pass-1');
  ok('an unreachable directory says so', r.status === 400 && /cannot be reached/.test(JSON.stringify(r.data)), r.data);
  ok('local accounts still sign in', (await login('admin@acme.test', 'supersecret1')).status === 200 && (await login(email('olga'), 'password123')).status === 200);
  await admin.put('/directory/ldap', { ...ldapCfg, enabled: false });
  ok('turned off: unknown emails are just wrong', (await login(email('zed'), 'whatever1')).status === 401);

  }

  // ---- single sign-on ----
  const ssoCfg = { enabled: true, issuer: oidc.issuer, client_id: oidc.clientId, button_label: `Acme SSO ${stamp}`, allowed_domains: [], auto_create: false };
  ok('sign-on needs a secret to turn on', (await admin.put('/directory/sso', ssoCfg)).status === 400);
  ok('the issuer must be https', (await admin.put('/directory/sso', { ...ssoCfg, issuer: 'http://idp.example.com', client_secret: 'x' })).status === 400);
  ok('domains are checked', (await admin.put('/directory/sso', { ...ssoCfg, allowed_domains: ['not a domain'], client_secret: 'x' })).status === 400);
  ok('before: no button', (await new Client().get('/auth/sso')).data.enabled === false);
  r = await admin.put('/directory/sso', { ...ssoCfg, client_secret: oidc.clientSecret });
  ok('sign-on saved', r.status === 200 && r.data.sso_has_secret && r.data.sso_redirect_uri.endsWith('/api/auth/sso/callback') && !JSON.stringify(r.data).includes(oidc.clientSecret), r.data);
  ok('the provider check', (await admin.post('/directory/sso/test')).data.ok === true);
  r = await new Client().get('/auth/sso');
  ok('the login page gets the button', r.data.enabled === true && r.data.label === `Acme SSO ${stamp}`, r.data);

  // The browser's round trip, by hand: start -> provider -> callback -> app.
  const manual = (url: string, cookie?: string) => fetch(url, { redirect: 'manual', headers: cookie ? { cookie } : {} });
  const roundTrip = async () => {
    const start = await manual(`${API}/auth/sso/start`);
    const toProvider = start.headers.get('location') ?? '';
    if (!toProvider.startsWith(oidc.issuer)) return { start: toProvider };
    const back = (await manual(toProvider)).headers.get('location') ?? '';
    const done = await manual(back);
    return { toProvider, back, location: done.headers.get('location') ?? '', cookie: (done.headers.get('set-cookie') ?? '').split(';')[0] };
  };
  const signedIn = async (t: { cookie?: string }) => {
    if (!t.cookie) return null;
    const c = new Client(); c.cookie = t.cookie;
    const r = await c.post('/auth/refresh');
    return r.status === 200 ? c : null;
  };
  const ssoError = (t: { location?: string; start?: string }) => {
    const l = t.location ?? t.start ?? '';
    return l.startsWith('/login?') ? new URL(l, 'http://x').searchParams.get('sso_error') ?? '' : '';
  };

  oidc.as = { sub: `sub-ann-${stamp}`, email: email('ann'), given_name: 'Ann', family_name: 'Sso', email_verified: true };
  let t: any = await roundTrip();
  ok('to the provider with PKCE, a nonce and our callback', (() => { const u = new URL(t.toProvider); return u.searchParams.get('code_challenge_method') === 'S256' && u.searchParams.get('nonce') && u.searchParams.get('redirect_uri') === 'http://localhost:8080/api/auth/sso/callback'; })(), t);
  ok('no account and no auto-create: refused', /no employee account/.test(ssoError(t)), t);
  const ann = (await admin.post('/users', { email: email('ann'), password: 'password123', first_name: 'Ann', last_name: 'Local' })).data;
  t = await roundTrip();
  let c = await signedIn(t);
  ok('an existing account by email: signed in', t.location === '/' && c && (await c.get('/auth/me')).data.id === ann.id, t);
  ok('a replayed callback is refused', /already used|took too long/.test(ssoError({ location: (await manual(t.back)).headers.get('location') ?? '' })));

  await admin.put('/directory/sso', { ...ssoCfg, auto_create: true });
  oidc.as = { sub: `sub-ned-${stamp}`, email: email('ned'), name: 'Ned Newcomer', email_verified: true };
  t = await roundTrip();
  c = await signedIn(t);
  const ned = c && (await c.get('/auth/me')).data;
  ok('auto-create: a new account', ned && ned.email === email('ned') && ned.first_name === 'Ned' && ned.last_name === 'Newcomer', ned ?? t);
  ok('created as an employee who cannot use a password', (await login(email('ned'), '!sso')).status === 401 && (await new Client().post('/auth/forgot-password', { email: email('ned') })).status === 200);
  oidc.as = { sub: `sub-ned-${stamp}`, email: email('ned2'), name: 'Ned Newcomer', email_verified: true };
  c = await signedIn(await roundTrip());
  ok('matched by subject when the email changes', c && (await c.get('/auth/me')).data.id === ned?.id);

  oidc.emailOnlyInUserinfo = true;
  oidc.as = { sub: `sub-ann-${stamp}`, email: email('ann'), email_verified: true };
  c = await signedIn(await roundTrip());
  oidc.emailOnlyInUserinfo = false;
  ok('the email from userinfo', c && (await c.get('/auth/me')).data.id === ann.id);

  await admin.put('/directory/sso', { ...ssoCfg, auto_create: true, allowed_domains: ['example.org'] });
  t = await roundTrip();
  ok('other domains are refused', /acme\.test cannot sign in/.test(ssoError(t)), t);
  await admin.put('/directory/sso', { ...ssoCfg, auto_create: true, allowed_domains: ['acme.test'] });
  ok('allowed domains sign in', !!(await signedIn(await roundTrip())));

  oidc.tamper = (cl) => { cl.nonce = 'forged'; };
  ok('a wrong nonce is refused', /does not belong/.test(ssoError(await roundTrip())));
  oidc.tamper = (cl) => { cl.aud = 'someone-else'; };
  ok('another audience is refused', /different application/.test(ssoError(await roundTrip())));
  oidc.tamper = (cl) => { cl.iss = 'http://localhost:1'; };
  ok('another issuer is refused', /different issuer/.test(ssoError(await roundTrip())));
  oidc.tamper = (cl) => { cl.exp = Math.floor(Date.now() / 1000) - 3600; };
  ok('an expired token is refused', /expired/.test(ssoError(await roundTrip())));
  oidc.tamper = (cl) => { cl.email_verified = false; };
  ok('an unverified email is refused', /not verified/.test(ssoError(await roundTrip())));
  oidc.deny = 'User cancelled';
  ok('the provider\'s refusal is shown', ssoError(await roundTrip()) === 'User cancelled');
  ok('a callback without a sign-in is refused', /took too long|not completed/.test(ssoError({ location: (await manual(`${API}/auth/sso/callback?code=x&state=made-up`)).headers.get('location') ?? '' })));

  await admin.patch(`/users/${ann.id}`, { is_active: false });
  oidc.as = { sub: `sub-ann-${stamp}`, email: email('ann'), email_verified: true };
  ok('a deactivated account is refused', /deactivated/.test(ssoError(await roundTrip())));

  await admin.put('/directory/sso', { ...ssoCfg, enabled: false });
  ok('turned off: no button', (await new Client().get('/auth/sso')).data.enabled === false);
  ok('turned off: start goes back to the login page', /not set up/.test(ssoError(await roundTrip())));
} finally {
  // Back to the defaults.
  await admin.put('/directory/ldap', { enabled: false, bind_password: '' });
  await admin.put('/directory/sso', { enabled: false, issuer: '', client_id: '', client_secret: '' });
  stop();
  await oidc.close();
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
