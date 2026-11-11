// Every private endpoint against every kind of missing or broken credential, one check per
// endpoint and kind: no token, garbage, an unsigned token naming the admin ("alg": "none"),
// an altered payload, a wrong signature, the token of a signed-out session, of a deactivated
// person, the token in the address instead of the header, the refresh cookie alone, the
// password as Basic auth, and an API-key-shaped string. All must answer 401.
import { admin, person, ok, check, finish, counts } from './lib.ts';
import { endpoints, isPublic, call, fillPath, NIL } from './sweep.ts';
import { Client } from '../e2e/lib.ts';

const b64 = (o: unknown) => Buffer.from(JSON.stringify(o)).toString('base64url');
const adminId = (await admin.get('/auth/me')).data.id;
const vic = await person('Vic');
const [h, p, s] = vic.token!.split('.');
const claims = JSON.parse(Buffer.from(p, 'base64url').toString());
const out = await person('Otto');
const outToken = out.token!, outCookie = out.cookie ?? '';
await out.post('/auth/logout');
const gone = await person('Dora');
const goneToken = gone.token!;
await admin.patch(`/users/${gone.id}`, { is_active: false });
const fresh = new Client();
await fresh.post('/auth/login', { email: vic.email, password: 'password123' });

const kinds: [string, (path: string) => { token?: string; headers?: Record<string, string>; path?: string }][] = [
  ['no token', () => ({})],
  ['garbage', () => ({ token: 'not.a.token' })],
  ['"alg": "none" as the admin', () => ({ token: `${b64({ alg: 'none', typ: 'JWT' })}.${b64({ ...claims, sub: adminId })}.` })],
  ['an altered payload (the admin)', () => ({ token: `${h}.${b64({ ...claims, sub: adminId })}.${s}` })],
  ['a wrong signature', () => ({ token: `${h}.${p}.${Buffer.from('x'.repeat(32)).toString('base64url')}` })],
  ['a signed-out session', () => ({ token: outToken })],
  ['a deactivated person', () => ({ token: goneToken })],
  ['the token in the address', (path) => ({ path: `${path}${path.includes('?') ? '&' : '?'}access_token=${encodeURIComponent(vic.token!)}&token=${encodeURIComponent(vic.token!)}` })],
  ['the refresh cookie alone', () => ({ headers: { cookie: fresh.cookie ?? outCookie } })],
  ['Basic auth with the password', () => ({ headers: { authorization: 'Basic ' + Buffer.from(`${vic.email}:password123`).toString('base64') } })],
  ['an API-key-shaped string', () => ({ token: 'nerp_live_0000000000000000000000000000' })],
];

let n = 0;
for (const e of endpoints()) {
  if (isPublic(e.path)) continue;
  n++;
  const path = fillPath(e.path, [NIL]);
  const body = e.method === 'GET' || e.method === 'DELETE' ? undefined : '{}';
  for (const [kind, make] of kinds) {
    const k = make(path);
    const r = await call(e.method, k.path ?? path, { token: k.token, headers: k.headers, body });
    check(`${e.method} ${e.path} with ${kind} → 401`, r.status === 401, r.status);
  }
}
ok(`(${n} private endpoints × ${kinds.length} kinds of bad credentials swept)`, n > 800);
console.log(`checks so far: ${JSON.stringify(counts())}`);
await finish();
