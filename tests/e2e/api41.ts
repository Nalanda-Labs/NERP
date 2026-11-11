// Two-step sign-in under attack: a code can't be used twice; one sign-in attempt can't be used
// for another person's or for the other step; guessing codes ends the attempt; recovery codes
// work once; hostile mobile numbers are refused cleanly; someone deactivated between steps
// doesn't get in; an administrator's reset makes them set it up again; a session can't skip it.
import { API, Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { totp } from './totp.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 700)); };
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const email = (n: string) => `${n}.${stamp}@acme.test`;
const ids: Record<string, string> = {};
for (const n of ['ivy', 'jon', 'kim', 'lou', 'max']) ids[n] = (await admin.post('/users', { email: email(n), password: 'password123', first_name: n, last_name: 'Attack' })).data.id;
const login = async (n: string) => { const c = new Client(); const r = await c.post('/auth/login', { email: email(n), password: 'password123' }); return Object.assign(c, { status: r.status, data: r.data }); };
const settings = (await admin.get('/settings')).data;
const now = () => Date.now() / 1000;
/** Signs up for two-step sign-in during login; returns the secret and the recovery codes. */
const enroll = async (n: string) => {
  const c = await login(n);
  const secret = (await new Client().post('/auth/login/2fa-setup/start', { challenge: c.data.challenge })).data.secret;
  const r = await new Client().post('/auth/login/2fa-setup', { challenge: c.data.challenge, mobile: '+1 555 0100', code: totp(secret) });
  return { secret, codes: r.data.recovery_codes as string[] };
};
const raw = (path: string, body: string, type = 'application/json') => fetch(API + path, { method: 'POST', headers: { 'content-type': type }, body });

try {
  await admin.put('/settings', { ...settings, require_two_factor: true });
  const ivy = await enroll('ivy');
  ok('set up', ivy.secret && ivy.codes?.length === 10);

  // ---- one code, one use ----
  const a = await login('ivy');
  const b = await login('ivy');
  const code = totp(ivy.secret, now() + 30);
  ok('a code works once', (await a.post('/auth/login/2fa', { challenge: a.data.challenge, code })).status === 200);
  ok('and not again, even in another attempt', (await b.post('/auth/login/2fa', { challenge: b.data.challenge, code })).status === 401);
  ok('an attempt is used up by signing in', (await a.post('/auth/login/2fa', { challenge: a.data.challenge, code: totp(ivy.secret, now() - 30) })).status === 401);

  // ---- attempts are not interchangeable ----
  await enroll('jon');
  const jonAttempt = await login('jon');
  ok('someone else\'s attempt with my code: refused', (await new Client().post('/auth/login/2fa', { challenge: jonAttempt.data.challenge, code: totp(ivy.secret, now() + 60) })).status === 401);
  const kim = await login('kim');
  ok('a setup attempt is not a code attempt', (await new Client().post('/auth/login/2fa', { challenge: kim.data.challenge, code: '123456' })).status === 401);
  ok('a code attempt is not a setup attempt', (await new Client().post('/auth/login/2fa-setup/start', { challenge: jonAttempt.data.challenge })).status === 401);
  const kim2 = await login('kim');
  const s1 = (await new Client().post('/auth/login/2fa-setup/start', { challenge: kim.data.challenge })).data.secret;
  const s2 = (await new Client().post('/auth/login/2fa-setup/start', { challenge: kim2.data.challenge })).data.secret;
  ok('each attempt has its own key', s1 && s2 && s1 !== s2);
  ok('a code for one attempt\'s key doesn\'t finish another', (await new Client().post('/auth/login/2fa-setup', { challenge: kim2.data.challenge, mobile: '+1 555 0100', code: totp(s1) })).status === 400);

  // ---- guessing ----
  const guess = await login('ivy');
  const tries: number[] = [];
  for (let i = 0; i < 6; i++) tries.push((await guess.post('/auth/login/2fa', { challenge: guess.data.challenge, code: String(100000 + i) })).status);
  ok('five wrong codes end the attempt', tries.slice(0, 5).every((s) => s === 401) && tries[5] === 401);
  ok('even the right code is refused then', (await guess.post('/auth/login/2fa', { challenge: guess.data.challenge, code: totp(ivy.secret, now() - 30) })).status === 401);
  const parallel = await login('ivy');
  const burst = await Promise.all(Array.from({ length: 12 }, (_, i) => new Client().post('/auth/login/2fa', { challenge: parallel.data.challenge, code: String(200000 + i) })));
  ok('guessing in parallel gets nowhere', burst.every((r) => r.status === 401 || r.status === 429));
  ok('and the attempt is gone', [401, 429].includes((await new Client().post('/auth/login/2fa', { challenge: parallel.data.challenge, code: totp(ivy.secret) })).status));

  // ---- recovery codes ----
  const r1 = await login('ivy');
  ok('a recovery code works', (await r1.post('/auth/login/2fa', { challenge: r1.data.challenge, code: ivy.codes[1] })).status === 200);
  const r2 = await login('ivy');
  ok('once', (await r2.post('/auth/login/2fa', { challenge: r2.data.challenge, code: ivy.codes[1] })).status === 401);
  const r3 = await login('ivy');
  ok('in any case and with spaces', (await r3.post('/auth/login/2fa', { challenge: r3.data.challenge, code: ` ${ivy.codes[2].toUpperCase()} ` })).status === 200);
  const r4 = await login('jon');
  ok('only your own', (await r4.post('/auth/login/2fa', { challenge: r4.data.challenge, code: ivy.codes[3] })).status === 401);

  // ---- hostile input ----
  const lou = await login('lou');
  await new Client().post('/auth/login/2fa-setup/start', { challenge: lou.data.challenge });
  const bad = ['', '+', '++44 7700 900123', '44+7700900123', '+44 7700 900123 ext 5', '١٢٣٤٥٦٧٨٩', '+1 555 0100 0000 0000 0000', 'x'.repeat(10_000), '+1\n555 0100', '+44 7700 900123 ✓'];
  const results: number[] = [];
  for (const m of bad) results.push((await new Client().post('/auth/login/2fa-setup', { challenge: lou.data.challenge, mobile: m, code: '000000' })).status);
  ok('bad mobile numbers are refused, not crashed on', results.every((s) => s === 400), results);
  ok('a mobile number that isn\'t text', (await new Client().post('/auth/login/2fa-setup', { challenge: lou.data.challenge, mobile: 447700900123, code: '000000' })).status >= 400);
  ok('malformed bodies', (await raw('/auth/login/2fa-setup', '{"challenge":')).status >= 400 && (await raw('/auth/login/2fa', 'challenge=x&code=1', 'application/x-www-form-urlencoded')).status >= 400);
  ok('a huge code', (await new Client().post('/auth/login/2fa', { challenge: 'x', code: '1'.repeat(100_000) })).status >= 400);
  ok('the server is fine', (await new Client().post('/auth/login', { email: email('ivy'), password: 'password123' })).status === 200);

  // ---- deactivated between the steps ----
  const max = await login('max');
  await new Client().post('/auth/login/2fa-setup/start', { challenge: max.data.challenge });
  const maxSecret = (await new Client().post('/auth/login/2fa-setup/start', { challenge: max.data.challenge })).data.secret;
  await admin.patch(`/users/${ids.max}`, { is_active: false });
  const done = await new Client().post('/auth/login/2fa-setup', { challenge: max.data.challenge, mobile: '+1 555 0100', code: totp(maxSecret) });
  ok('deactivated during setup: not signed in', done.status !== 200 || !done.data.access_token, done.data);
  const ivyAttempt = await login('ivy');
  await admin.patch(`/users/${ids.ivy}`, { is_active: false });
  ok('deactivated before the code: not signed in', (await new Client().post('/auth/login/2fa', { challenge: ivyAttempt.data.challenge, code: totp(ivy.secret, now() + 60) })).status !== 200);
  await admin.patch(`/users/${ids.ivy}`, { is_active: true });

  // ---- an administrator's reset ----
  ok('an administrator resets it', (await admin.post(`/users/${ids.jon}/2fa/reset`)).status === 200);
  const again = await login('jon');
  ok('and it must be set up again', again.data.two_factor_setup_required === true, again.data);

  // ---- no way around it ----
  const half = await login('ivy');
  ok('no session before the code', !half.cookie && (await half.post('/auth/refresh')).status === 401);
  ok('a password change needs a session', (await half.post('/auth/change-password', { current_password: 'password123', new_password: 'password456' })).status === 401);
} finally {
  await admin.put('/settings', { ...settings, require_two_factor: false });
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
