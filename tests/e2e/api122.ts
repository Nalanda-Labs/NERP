// Sandbox: the live system copies itself into the sandbox database and anonymises it; a second
// server in sandbox mode runs on the copy, says so, signs employees in, keeps its emails
// instead of sending them and refuses backups.
import { spawn } from 'node:child_process';
import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { dbUrl } from '../db/cleanup.ts';
import pg from 'pg';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const s = Date.now().toString(36);
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
const sandboxUrl = dbUrl!.replace(/\/([^/?]+)(\?.*)?$/, '/$1_sandbox_test$2');
const SB = 'http://127.0.0.1:8092';
let server: ReturnType<typeof spawn> | null = null;
const email = `real.${s}@example.com`;

try {
  let r = await admin.get('/admin/sandbox');
  ok('the live system can make a sandbox', r.status === 200 && r.data.configured === true && r.data.is_sandbox === false && r.data.url === SB, r.data);
  ok('the live system isn\'t one', (await admin.get('/public/instance')).data.sandbox === false);

  // Real client data that must not reach the copy.
  const c = (await admin.post('/crm/contact/items', { first_name: 'Rosalind', last_name: `Real${s}`, address: { line1: '12 Secret Lane', city: 'Leeds', country: 'GB' }, multifields: [{ kind: 'email', value_type: 'work', value: email }, { kind: 'phone', value_type: 'work', value: '+44 113 496 0000' }] })).data;
  await db.query("INSERT INTO crm_timeline (item_id, kind, body) VALUES ($1, 'comment', $2)", [c.id, `Rosalind's private note ${s}`]);
  await db.query("INSERT INTO crm_emails (direction, status, message_id, thread_id, subject, from_addr, sent_at) VALUES ('in', 'received', $1, gen_random_uuid(), 'Private', $2, now())", [`<sb-${s}@x>`, email]);

  const before = (await admin.get('/admin/sandbox')).data.last?.finished_at ?? '';
  r = await admin.post('/admin/sandbox/refresh', {});
  ok('refreshing starts', r.status === 200 && r.data.started === true, r.data);
  ok('…once at a time', (await admin.post('/admin/sandbox/refresh', {})).status === 409);
  let last: any = null;
  for (let i = 0; i < 300; i++) {
    const x = (await admin.get('/admin/sandbox')).data;
    if (!x.running && x.last && x.last.finished_at !== before) { last = x.last; break; }
    await sleep(1000);
  }
  ok('the copy is made', last?.ok === true && /anonymis|renamed/.test(last.output), last);

  const sdb = new pg.Client({ connectionString: sandboxUrl });
  await sdb.connect();
  const sc = (await sdb.query('SELECT first_name, last_name, title, address FROM crm_items WHERE id = $1', [c.id])).rows[0];
  ok('the contact is renamed', sc && sc.first_name === 'Contact' && !JSON.stringify(sc).includes('Rosalind') && sc.address?.city === 'Leeds' && !sc.address?.line1, sc);
  ok('no trace of their email or phone', (await sdb.query("SELECT 1 FROM crm_multifields WHERE value = $1 OR normalized LIKE '%1134960000%'", [email])).rowCount === 0);
  ok('…nor of the note or the emails', (await sdb.query("SELECT 1 FROM crm_timeline WHERE body LIKE $1", [`%${s}%`])).rowCount === 0 && (await sdb.query('SELECT COUNT(*)::int AS n FROM crm_emails')).rows[0].n === 0);
  ok('keys, sessions and file contents are gone', (await sdb.query('SELECT (SELECT COUNT(*) FROM api_keys) + (SELECT COUNT(*) FROM auth_sessions) + (SELECT COUNT(*) FROM files WHERE storage_path NOT LIKE \'.sandbox/%\') AS n')).rows[0].n === '0');
  ok('employees and the pipeline stay', (await sdb.query("SELECT 1 FROM users WHERE email = 'admin@acme.test'")).rowCount === 1 && (await sdb.query('SELECT COUNT(*)::int AS n FROM crm_stages')).rows[0].n > 0);
  ok('the copy knows what it is', (await sdb.query("SELECT value FROM settings WHERE key = 'sandbox_of'")).rows[0]?.value?.copied_at);
  await sdb.end();

  // ---- a server in sandbox mode on the copy ----
  const uploads = mkdtempSync(join(tmpdir(), 'nerp-sandbox-'));
  server = spawn('./target/debug/nerp', [], {
    cwd: new URL('../../backend/', import.meta.url).pathname,
    env: { ...process.env, SANDBOX: 'true', DATABASE_URL: sandboxUrl, BIND_ADDR: '127.0.0.1:8092', PUBLIC_URL: SB, REDIS_URL: 'redis://127.0.0.1:6379/1', UPLOAD_DIR: uploads, STATIC_DIR: '', WORKER_ENABLED: 'true', COOKIE_SECURE: 'false', SANDBOX_DATABASE_URL: '' },
    stdio: 'ignore',
  });
  let up = false;
  for (let i = 0; i < 120 && !up; i++) { up = await fetch(`${SB}/api/health`).then((x) => x.ok, () => false); if (!up) await sleep(500); }
  ok('the sandbox server starts on the copy', up);
  const sb = await (await fetch(`${SB}/api/public/instance`)).json();
  ok('it says it is a sandbox, of what', sb.sandbox === true && sb.copy?.copied_at, sb);
  const login = await fetch(`${SB}/api/auth/login`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ email: 'admin@acme.test', password: 'supersecret1' }) });
  const token = (await login.json()).access_token;
  ok('employees sign in as usual', login.status === 200 && !!token);
  const call = (method: string, path: string, body?: unknown) => fetch(`${SB}/api${path}`, { method, headers: { authorization: `Bearer ${token}`, 'content-type': 'application/json' }, body: body ? JSON.stringify(body) : undefined });
  const t = await call('POST', '/admin/email/test', { to: `nobody.${s}@example.com` });
  ok('an email can be "sent"', t.status === 200);
  let kept = false;
  for (let i = 0; i < 30 && !kept; i++) {
    const o = await (await call('GET', '/admin/sandbox/outbox')).json();
    kept = o.items?.some((m: any) => m.to === `nobody.${s}@example.com`);
    if (!kept) await sleep(500);
  }
  ok('…it stays in the sandbox\'s outbox', kept);
  const sdb2 = new pg.Client({ connectionString: sandboxUrl });
  await sdb2.connect();
  let status = '';
  for (let i = 0; i < 30 && status !== 'done'; i++) { status = (await sdb2.query("SELECT status FROM jobs WHERE kind = 'email.send' AND payload->>'to' = $1", [`nobody.${s}@example.com`])).rows[0]?.status ?? ''; if (status !== 'done') await sleep(500); }
  await sdb2.end();
  ok('…marked done without reaching a mail server', status === 'done', status);
  ok('backups aren\'t run from a sandbox', (await call('POST', '/admin/backups', {})).status === 400);
  ok('a sandbox doesn\'t refresh itself', (await call('POST', '/admin/sandbox/refresh', {})).status === 400);

  const u = new Client();
  await admin.post('/users', { email: `sbx.${s}@acme.test`, password: 'password123', first_name: 'Sb', last_name: `X${s}` });
  await u.post('/auth/login', { email: `sbx.${s}@acme.test`, password: 'password123' });
  ok('the sandbox page is for system managers', (await u.get('/admin/sandbox')).status === 403 && (await u.post('/admin/sandbox/refresh', {})).status === 403);
} finally {
  if (server) { server.kill('SIGTERM'); await sleep(1500); }
  await db.query('DELETE FROM crm_emails WHERE message_id = $1', [`<sb-${s}@x>`]).catch(() => {});
  await db.query('DELETE FROM crm_items WHERE last_name = $1', [`Real${s}`]).catch(() => {});
  await db.query(`DROP DATABASE IF EXISTS "${sandboxUrl.replace(/^.*\/([^/?]+)(\?.*)?$/, '$1')}" WITH (FORCE)`).catch((e) => console.log('cleanup:', e.message));
  await db.end();
  await cleanupRun(s);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
