// Required two-step sign-in: the company setting; at the next sign-in, people without an
// authenticator set it up (mobile number, QR secret, a code) before they get in; recovery
// codes; later sign-ins ask for a code; it can't be turned off while required; the same
// after company sign-on (SSO); wrong codes and expired attempts.
import { API, BASE, Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { startMockOidc } from './mock-oidc.ts';
import { totp } from './totp.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 700)); };
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const email = (n: string) => `${n}.${stamp}@acme.test`;
for (const n of ['erin', 'finn', 'gus']) await admin.post('/users', { email: email(n), password: 'password123', first_name: n[0].toUpperCase() + n.slice(1), last_name: 'Twostep' });
const login = async (e: string, password = 'password123') => { const c = new Client(); const r = await c.post('/auth/login', { email: e, password }); return Object.assign(c, { status: r.status, data: r.data }); };
const settings = (await admin.get('/settings')).data;
const oidc = await startMockOidc(9479);

try {
  ok('off by default: straight in', (await login(email('erin'))).data.access_token);
  const erinEarly = await login(email('erin'));
  ok('only administrators change it', (await erinEarly.put('/settings', { ...settings, require_two_factor: true })).status === 403);
  ok('it is a switch', (await admin.put('/settings', { ...settings, require_two_factor: 'yes' })).status === 400);
  let r = await admin.put('/settings', { ...settings, require_two_factor: true });
  ok('turned on', r.status === 200 && r.data.require_two_factor === true, r.data);

  // ---- setting it up at sign-in ----
  let c = await login(email('erin'));
  ok('the password is not enough any more', c.status === 200 && c.data.two_factor_setup_required === true && c.data.challenge && !c.data.access_token, c.data);
  ok('and gives no session', (await c.get('/auth/me')).status === 401);
  const challenge = c.data.challenge;
  ok('a made-up attempt is refused', (await new Client().post('/auth/login/2fa-setup/start', { challenge: 'nope' })).status === 401);
  r = await new Client().post('/auth/login/2fa-setup/start', { challenge });
  const secret = r.data.secret;
  ok('the QR code and key', r.status === 200 && r.data.qr_png_base64.length > 100 && /^[A-Z2-7]+=*$/.test(secret) && r.data.otpauth_url.startsWith('otpauth://totp/') && r.data.mobile === null, r.data);
  ok('asking again shows the same key', (await new Client().post('/auth/login/2fa-setup/start', { challenge })).data.secret === secret);
  ok('a mobile number is required', (await new Client().post('/auth/login/2fa-setup', { challenge, mobile: '', code: totp(secret) })).status === 400);
  ok('and must look like one', (await new Client().post('/auth/login/2fa-setup', { challenge, mobile: 'call me', code: totp(secret) })).status === 400);
  ok('a wrong code is refused', (await new Client().post('/auth/login/2fa-setup', { challenge, mobile: '+44 7700 900123', code: '000000' })).status === 400);
  const erin = new Client();
  r = await erin.post('/auth/login/2fa-setup', { challenge, mobile: '+44 7700 900123', code: totp(secret) });
  ok('set up: signed in', r.status === 200 && r.data.access_token && r.data.user.email === email('erin'), r.data);
  ok('with ten recovery codes', r.data.recovery_codes?.length === 10);
  const codes: string[] = r.data.recovery_codes;
  r = await erin.get('/auth/me');
  ok('the mobile number is on the profile, and two-step is on', r.data.mobile === '+44 7700 900123' && r.data.two_factor_enabled === true, r.data);
  ok('the refresh cookie works', (await erin.post('/auth/refresh')).status === 200);
  ok('the attempt is used up', (await new Client().post('/auth/login/2fa-setup', { challenge, mobile: '+44 7700 900123', code: totp(secret) })).status === 401);

  // ---- later sign-ins ----
  c = await login(email('erin'));
  ok('next time: a code is asked for', c.data.two_factor_required === true && !c.data.access_token, c.data);
  r = await c.post('/auth/login/2fa', { challenge: c.data.challenge, code: totp(secret, Date.now() / 1000 + 30) });
  ok('a code from the app signs in', r.status === 200 && r.data.access_token, r.data);
  c = await login(email('erin'));
  r = await c.post('/auth/login/2fa', { challenge: c.data.challenge, code: codes[0] });
  ok('so does a recovery code', r.status === 200 && r.data.access_token, r.data);
  ok('it cannot be turned off while required', (await erin.post('/auth/2fa/disable', { password: 'password123' })).status === 400);

  // ---- wrong codes during setup ----
  c = await login(email('finn'));
  const fch = c.data.challenge;
  await new Client().post('/auth/login/2fa-setup/start', { challenge: fch });
  let last = 0;
  for (let i = 0; i < 5; i++) last = (await new Client().post('/auth/login/2fa-setup', { challenge: fch, mobile: '+1 555 0100', code: '123456' })).status;
  ok('five wrong codes end the attempt', last === 401 && (await new Client().post('/auth/login/2fa-setup/start', { challenge: fch })).status === 401);
  ok('a wrong password still fails first', (await login(email('finn'), 'nope-nope')).status === 401);

  // ---- after company sign-on ----
  await admin.put('/directory/sso', { enabled: true, issuer: oidc.issuer, client_id: oidc.clientId, client_secret: oidc.clientSecret, auto_create: false });
  const manual = (url: string) => fetch(url, { redirect: 'manual' });
  const sso = async () => {
    const toProvider = (await manual(`${API}/auth/sso/start`)).headers.get('location')!;
    const back = (await manual(toProvider)).headers.get('location')!;
    const done = await manual(back);
    return { location: done.headers.get('location') ?? '', cookie: done.headers.get('set-cookie') };
  };
  oidc.as = { sub: `sub-gus-${stamp}`, email: email('gus'), email_verified: true };
  let t = await sso();
  let u = new URL(t.location, BASE);
  ok('sign-on: then setting up the code', u.pathname === '/login' && u.searchParams.get('step') === 'setup' && u.searchParams.get('challenge') && !t.cookie, t);
  const gch = u.searchParams.get('challenge')!;
  const gsecret = (await new Client().post('/auth/login/2fa-setup/start', { challenge: gch })).data.secret;
  r = await new Client().post('/auth/login/2fa-setup', { challenge: gch, mobile: '+49 151 23456789', code: totp(gsecret) });
  ok('and then in', r.status === 200 && r.data.user.email === email('gus'));
  t = await sso();
  u = new URL(t.location, BASE);
  ok('sign-on with a code set up: the code is asked for', u.searchParams.get('step') === 'code' && !t.cookie, t);
  r = await new Client().post('/auth/login/2fa', { challenge: u.searchParams.get('challenge'), code: totp(gsecret, Date.now() / 1000 + 30) });
  ok('and then in', r.status === 200 && r.data.user.email === email('gus'));

  // ---- turned off again ----
  await admin.put('/settings', { ...settings, require_two_factor: false });
  ok('off: people without a code go straight in', !!(await login(email('finn'))).data.access_token);
  ok('people with one are still asked', (await login(email('erin'))).data.two_factor_required === true);
  ok('and may turn it off now', (await erin.post('/auth/2fa/disable', { password: 'password123' })).status === 200);
} finally {
  await admin.put('/settings', { ...settings, require_two_factor: false });
  await admin.put('/directory/sso', { enabled: false, issuer: '', client_id: '', client_secret: '' });
  await oidc.close();
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
