// Sign-ins outlive Redis: sessions, refresh tokens and emailed links are kept in PostgreSQL
// (Redis holds a copy). With Redis's copies wiped, a signed-in browser keeps working and
// refreshing, the session list is intact, a password-reset link still works; a session that
// was signed out stays signed out; a refresh token used twice still ends the session (theft);
// and the System page says whether Redis keeps its data.
import { BASE, Client, waitFor } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 700)); };
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const s = Date.now().toString(36);
const env = readFileSync(new URL('../../backend/.env', import.meta.url), 'utf8');
const redisUrl = process.env.REDIS_URL ?? (/^REDIS_URL=(.*)$/m.exec(env)?.[1] ?? 'redis://127.0.0.1:6379').trim();
/** Wipes Redis's copies of every session and every sign-out marker (as a Redis restart would). */
const wipe = () => {
  for (const pattern of ['auth:sess:*', 'auth:revoked_sid:*']) {
    execFileSync('sh', ['-c', 'redis-cli -u "$1" --scan --pattern "$2" | xargs -r -n 500 redis-cli -u "$1" UNLINK > /dev/null', 'wipe', redisUrl, pattern]);
  }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const email = `ann.${s}@acme.test`;
await admin.post('/users', { email, password: 'password123', first_name: 'Ann', last_name: `Durable${s}` });

/** A browser's sign-in: its refresh cookie and access token. */
async function signIn() {
  const r = await fetch(`${BASE}/api/auth/login`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ email, password: 'password123' }) });
  const cookie = (r.headers.getSetCookie() ?? []).find((c) => c.startsWith('nerp_rt='))!.split(';')[0];
  return { cookie, access: (await r.json()).access_token as string };
}
const refresh = async (cookie: string) => {
  const r = await fetch(`${BASE}/api/auth/refresh`, { method: 'POST', headers: { cookie } });
  const next = (r.headers.getSetCookie() ?? []).find((c) => c.startsWith('nerp_rt='))?.split(';')[0];
  return { status: r.status, cookie: next, access: r.status === 200 ? (await r.json()).access_token as string : null };
};
const me = async (access: string) => (await fetch(`${BASE}/api/auth/me`, { headers: { authorization: `Bearer ${access}` } })).status;

try {
  const laptop = await signIn();
  const phone = await signIn();
  wipe();
  ok('after Redis loses its copies, the signed-in browser keeps working', (await me(laptop.access)) === 200);
  let r = await refresh(laptop.cookie);
  ok('…and refreshes (the refresh token is checked in the database)', r.status === 200 && !!r.cookie && (await me(r.access!)) === 200, r.status);
  const old = laptop.cookie;
  laptop.cookie = r.cookie!;
  laptop.access = r.access!;
  const list = await (await fetch(`${BASE}/api/auth/sessions`, { headers: { authorization: `Bearer ${laptop.access}` } })).json();
  // Redis's copy of a session lives minutes, not as long as the session: a copy left by a
  // sign-out Redis didn't hear about runs out quickly.
  const sid = JSON.parse(Buffer.from(laptop.access.split('.')[1], 'base64url').toString()).sid;
  const ttl = Number(execFileSync('redis-cli', ['-u', redisUrl, 'TTL', `auth:sess:${sid}`]).toString().trim());
  ok('Redis keeps a session\'s copy for at most five minutes', ttl > 0 && ttl <= 300, ttl);
  ok('the list of signed-in devices is intact', Array.isArray(list) && list.length === 2 && list.filter((x: any) => x.current).length === 1, list);

  // Sign the phone out, then lose Redis's sign-out marker: it must stay signed out.
  const phoneId = list.find((x: any) => !x.current).id;
  ok('signing out another device', (await fetch(`${BASE}/api/auth/sessions/${phoneId}`, { method: 'DELETE', headers: { authorization: `Bearer ${laptop.access}` } })).status === 200);
  wipe();
  ok('…it stays signed out after Redis loses the marker', (await me(phone.access)) === 401 && (await refresh(phone.cookie)).status === 401);

  // A refresh token used twice: soon after (two tabs) it's only refused; later, it's theft.
  ok('an exchanged refresh token used again at once: refused', (await refresh(old)).status === 401 && (await me(laptop.access)) === 200);
  await sleep(31_000);
  wipe();
  ok('…used again later: the session ends for everyone holding it', (await refresh(old)).status === 401 && (await me(laptop.access)) === 401 && (await refresh(laptop.cookie)).status === 401);

  // A password-reset link made before Redis lost everything.
  await fetch(`${BASE}/api/auth/forgot-password`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ email }) });
  const mail = await waitFor(async () => (await (await fetch(`http://localhost:8026/api/v1/search?query=${encodeURIComponent(`to:${email}`)}`)).json()).messages?.[0], 15000);
  const text: string = (await (await fetch(`http://localhost:8026/api/v1/message/${mail.ID}`)).json()).Text;
  const token = /reset-password\?token=([\w-]+)/.exec(text)?.[1];
  wipe();
  r = await fetch(`${BASE}/api/auth/reset-password`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ token, password: 'a-new-password-123' }) }) as any;
  ok('a password-reset link still works', (r as any).status === 200, await (r as any).text?.());
  ok('…once', (await fetch(`${BASE}/api/auth/reset-password`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ token, password: 'another-password-123' }) })).status === 400);
  ok('…and the new password signs in', (await fetch(`${BASE}/api/auth/login`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ email, password: 'a-new-password-123' }) })).status === 200);

  const sys = (await admin.get('/admin/system')).data.redis;
  ok('the System page says whether Redis keeps its data', typeof sys?.persistent === 'boolean' && typeof sys.aof === 'boolean', sys);
} finally {
  await cleanupRun(s);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
