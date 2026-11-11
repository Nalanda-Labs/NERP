// Shared helpers for the security suites: results, test people with chosen permissions,
// raw requests, and resetting the rate limits the attacks trip.
import { execFileSync } from 'node:child_process';
import { Client, BASE, API } from '../e2e/lib.ts';
import { cleanupRun } from '../e2e/cleanup.ts';

export { Client, BASE, API, cleanupRun };
export const stamp = Date.now().toString(36);

let pass = 0, fail = 0;
export const ok = (name: string, cond: unknown, extra: unknown = '') => {
  cond ? pass++ : fail++;
  console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 700));
};
/** A check in a sweep of thousands: counted like `ok`, but only failures are printed (the
 *  first 60, then a count). */
let shown = 0;
export const check = (name: string, cond: unknown, extra: unknown = '') => {
  if (cond) return void pass++;
  fail++;
  if (++shown <= 60) console.log('FAIL', name, String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 300));
  else if (shown === 61) console.log('FAIL … (more failures not shown)');
};
export const counts = () => ({ pass, fail });
export const finish = async () => {
  await cleanupRun(stamp);
  console.log(`\n${pass} passed, ${fail} failed`);
  process.exit(fail ? 1 : 0);
};

export const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const employeeRole = (await admin.get('/roles')).data.find((r: any) => r.code === 'employee');

/** A test employee (signed in), optionally with extra permissions. */
export async function person(first: string, perms: string[] = []): Promise<Client & { id: string; email: string }> {
  const email = `${first.toLowerCase()}.${stamp}@acme.test`;
  const u = (await admin.post('/users', { email, password: 'password123', first_name: first, last_name: 'Pentest' })).data;
  if (perms.length) {
    const role = (await admin.post('/roles', { code: `pen_${first.toLowerCase()}_${stamp}`, name: `Pentest ${first} ${stamp}`, permissions: perms })).data;
    await admin.put(`/users/${u.id}/roles`, { role_ids: [employeeRole.id, role.id] });
  }
  const c = new Client() as Client & { id: string; email: string };
  await c.post('/auth/login', { email, password: 'password123' });
  c.id = u.id;
  c.email = email;
  return c;
}

/** A request with full control over headers and body (no session handling). */
export async function raw(method: string, path: string, opts: { headers?: Record<string, string>; body?: string | Uint8Array | FormData; base?: string } = {}) {
  const res = await fetch((opts.base ?? API) + path, { method, headers: opts.headers, body: opts.body as BodyInit, redirect: 'manual' });
  const text = await res.text();
  let json: any = null;
  try { json = JSON.parse(text); } catch { /* not JSON */ }
  return { status: res.status, headers: res.headers, text, json };
}

/** Clears rate-limit counters in Redis (keys matching the pattern), after tripping them on purpose. */
export function resetLimits(pattern: string) {
  const url = process.env.REDIS_URL ?? 'redis://127.0.0.1:6379';
  const keys = execFileSync('redis-cli', ['-u', url, '--scan', '--pattern', pattern]).toString().split('\n').filter(Boolean);
  if (keys.length) execFileSync('redis-cli', ['-u', url, 'DEL', ...keys]);
}

/** Text that must never reach a client: database errors, panics, internal paths. */
export const LEAK = /syntax error|SQLSTATE|sqlx|panicked|stack backtrace|at src\/|\/home\/|RUST_BACKTRACE|unwrap\(\)|relation ".*" does not exist/i;
