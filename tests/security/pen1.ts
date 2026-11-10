// Sign-in and sessions: every private route refuses anonymous callers; forged, altered,
// signed-out and deactivated tokens are refused; a stolen refresh token used twice ends the
// session; brute force and password spraying are slowed; sign-in and password reset don't
// reveal which accounts exist.
import { routes, fill } from './routes.ts';
import { admin, person, raw, ok, finish, resetLimits, Client, stamp, BASE } from './lib.ts';

const b64 = (o: unknown) => Buffer.from(JSON.stringify(o)).toString('base64url');

// ---- anonymous callers ----
// The routes meant for people without an account: everything else must answer 401.
const PUBLIC = [
  /^\/api\/public\//, /^\/api\/health$/, /^\/api\/openapi\.json$/, /^\/api\/setup$/,
  /^\/api\/auth\/(login|login\/2fa|login\/2fa-setup|login\/2fa-setup\/start|logout|refresh|forgot-password|reset-password|accept-invite|invite\/\{token\})$/,
  /^\/api\/auth\/sso(\/start|\/callback)?$/, /^\/api\/crm\/mailboxes\/oauth\/callback$/, /^\/api\/realtime\/ws$/,
  /^\/api\/crm\/automation\/webhook\/\{token\}$/, /^\/api\/files\/\{id\}\/content$/,
];
const open: string[] = [];
for (const r of routes()) for (const m of r.methods) {
  if (PUBLIC.some((p) => p.test(r.path))) continue;
  const res = await raw(m, fill(r.path), { base: BASE, ...(m === 'GET' || m === 'DELETE' ? {} : { headers: { 'content-type': 'application/json' }, body: '{}' }) });
  if (res.status !== 401) open.push(`${m} ${r.path} → ${res.status}`);
}
ok(`every private route refuses anonymous callers (${routes().length} paths)`, open.length === 0, open);

// ---- tokens ----
const victim = await person('Vic');
const [h, p, s] = victim.token!.split('.');
const claims = JSON.parse(Buffer.from(p, 'base64url').toString());
const adminId = (await admin.get('/auth/me')).data.id ?? (await admin.get('/auth/me')).data.user?.id;
const call = (token: string) => raw('GET', '/auth/me', { headers: { authorization: `Bearer ${token}` } });
ok('the real token works', (await call(victim.token!)).status === 200);
ok('an altered payload (become the admin) is refused', (await call(`${h}.${b64({ ...claims, sub: adminId })}.${s}`)).status === 401);
ok('an unsigned token ("alg": "none") is refused', (await call(`${b64({ alg: 'none', typ: 'JWT' })}.${b64({ ...claims, sub: adminId })}.`)).status === 401);
ok('a token signed with another key is refused', (await call(`${h}.${p}.${Buffer.from('forged-signature-forged-signature').toString('base64url')}`)).status === 401);
ok('a token that has expired is refused', (await call(`${h}.${b64({ ...claims, exp: 1000 })}.${s}`)).status === 401);
ok('garbage is refused', (await call('not.a.token')).status === 401 && (await call('')).status === 401);
ok('an API-key-looking token is refused', (await call('nerp_live_0000000000000000000000000000')).status === 401);

// ---- signing out, password change, deactivation ----
const out = await person('Otto');
const outToken = out.token!;
await out.post('/auth/logout');
ok('after signing out, the access token is dead', (await call(outToken)).status === 401);
ok('…and the refresh cookie too', (await raw('POST', '/auth/refresh', { headers: { cookie: out.cookie ?? '' } })).status === 401);

const two = await person('Tess');
const other = new Client();
await other.post('/auth/login', { email: two.email, password: 'password123' });
await two.post('/auth/change-password', { current_password: 'password123', new_password: 'password456' });
ok('changing the password signs out the other devices', (await other.get('/auth/me')).status === 401);

const gone = await person('Dora');
await admin.patch(`/users/${gone.id}`, { is_active: false });
ok('a deactivated person\'s token is refused at once', (await gone.get('/auth/me')).status === 401);
ok('…and they cannot refresh', (await raw('POST', '/auth/refresh', { headers: { cookie: gone.cookie ?? '' } })).status === 401);

// ---- a stolen refresh token ----
const thief = await person('Theo');
const stolen = thief.cookie!;
let r = await raw('POST', '/auth/refresh', { headers: { cookie: stolen } });
ok('the refresh token works once', r.status === 200);
const fresh = r.headers.get('set-cookie')!.split(';')[0];
const freshAccess = r.json.access_token;
ok('the same refresh token is refused the second time', (await raw('POST', '/auth/refresh', { headers: { cookie: stolen } })).status === 401);
// Past the grace period for two tabs, a reused token means a copy is out there: the
// whole session ends, for the thief and the owner alike.
const { execFileSync } = await import('node:child_process');
const url = process.env.REDIS_URL ?? 'redis://127.0.0.1:6379';
const hash = (await import('node:crypto')).createHash('sha256').update(stolen.slice(stolen.indexOf('=') + 1)).digest('hex');
const used = execFileSync('redis-cli', ['-u', url, 'GET', `auth:rt_used:${hash}`]).toString().trim();
ok('the exchanged token is remembered', used.includes('|'), used);
execFileSync('redis-cli', ['-u', url, 'SET', `auth:rt_used:${hash}`, used.replace(/\|\d+$/, '|1000'), 'EX', '600']);
ok('reusing it later is refused…', (await raw('POST', '/auth/refresh', { headers: { cookie: stolen } })).status === 401);
ok('…and ends the session: the newer refresh token is dead', (await raw('POST', '/auth/refresh', { headers: { cookie: fresh } })).status === 401);
ok('…and so is its access token', (await call(freshAccess)).status === 401);
ok('it is in the audit log', (await admin.get('/audit?action=auth.refresh_reuse')).data.items?.some((a: any) => a.action === 'auth.refresh_reuse' && a.actor_id === thief.id), (await admin.get('/audit?action=auth.refresh_reuse')).data);

// ---- brute force, spraying and enumeration ----
const target = await person('Bruno');
let last = 0;
for (let i = 0; i < 11; i++) last = (await raw('POST', '/auth/login', { headers: { 'content-type': 'application/json' }, body: JSON.stringify({ email: target.email, password: `wrong-${i}` }) })).status;
ok('ten wrong passwords lock the account for a while', last === 429, last);
ok('…even the right password waits', (await raw('POST', '/auth/login', { headers: { 'content-type': 'application/json' }, body: JSON.stringify({ email: target.email, password: 'password123' }) })).status === 429);
resetLimits(`auth:fail:${target.email}`);

resetLimits('auth:failip:*');
const spray: number[] = [];
for (let i = 0; i < 101; i++) spray.push((await raw('POST', '/auth/login', { headers: { 'content-type': 'application/json' }, body: JSON.stringify({ email: `nobody${i}.${stamp}@acme.test`, password: 'Summer2026!' }) })).status);
ok('one password tried on 100 accounts: the address must wait', spray.at(-1) === 429 && spray.slice(0, 100).every((s) => s === 401), spray.slice(-3));
resetLimits('auth:failip:*');
resetLimits(`auth:fail:nobody*.${stamp}@acme.test`);

const unknown = await raw('POST', '/auth/login', { headers: { 'content-type': 'application/json' }, body: JSON.stringify({ email: `ghost.${stamp}@acme.test`, password: 'x' }) });
const wrong = await raw('POST', '/auth/login', { headers: { 'content-type': 'application/json' }, body: JSON.stringify({ email: target.email, password: 'x' }) });
ok('sign-in says the same for unknown accounts and wrong passwords', unknown.status === wrong.status && unknown.json?.error?.message === wrong.json?.error?.message, [unknown.json, wrong.json]);
const f1 = await raw('POST', '/auth/forgot-password', { headers: { 'content-type': 'application/json' }, body: JSON.stringify({ email: `ghost.${stamp}@acme.test` }) });
const f2 = await raw('POST', '/auth/forgot-password', { headers: { 'content-type': 'application/json' }, body: JSON.stringify({ email: target.email }) });
ok('password reset says the same for unknown accounts', f1.status === f2.status && f1.text === f2.text, [f1.text, f2.text]);
ok('a made-up reset token is refused', (await raw('POST', '/auth/reset-password', { headers: { 'content-type': 'application/json' }, body: JSON.stringify({ token: 'x'.repeat(43), password: 'password789' }) })).status === 400);
resetLimits('auth:failip:*');

await finish();
