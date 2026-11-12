// System → Health: the checks, the self-test, and alerts when a check turns red.
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { dbUrl } from '../db/cleanup.ts';
import pg from 'pg';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const s = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
const before = (await admin.get('/admin/health')).data.settings;
const alertTo = `alerts.${s}@outside.test`;
const keys = (x: any) => (x?.checks ?? []).map((c: any) => c.key);

try {
  let r = await admin.get('/admin/health');
  ok('the health page has its checks', r.status === 200 && ['database', 'redis', 'backups', 'pitr', 'encryption', 'certificate', 'jobs', 'mail', 'audit'].every((k) => keys(r.data.last).includes(k)), keys(r.data.last));
  const by = (x: any, k: string) => x.checks.find((c: any) => c.key === k);
  ok('…each with a level and a sentence', r.data.last.checks.every((c: any) => ['off', 'ok', 'warn', 'fail'].includes(c.level) && c.title && c.text), r.data.last.checks);
  ok('the database answers', by(r.data.last, 'database').level === 'ok' && /connections in use/.test(by(r.data.last, 'database').text));
  ok('a local address isn\'t checked for a certificate', by(r.data.last, 'certificate').level === 'off');
  ok('disk space is shown', keys(r.data.last).includes('disk_files') && /free of/.test(by(r.data.last, 'disk_files').text));

  r = await admin.post('/admin/health/run', {});
  const t = r.data.self_test;
  ok('the self-test writes and reads the database, Redis and file storage', ['test_database', 'test_redis', 'test_files'].every((k) => by(t, k)?.level === 'ok'), t?.checks);
  ok('…and reaches the mail server', by(t, 'test_mail') && ['ok', 'fail'].includes(by(t, 'test_mail').level), by(t, 'test_mail'));
  ok('…leaving nothing behind', (await db.query("SELECT 1 FROM settings WHERE key = 'health_probe'")).rowCount === 0);
  ok('the run is in the audit log', (await db.query("SELECT 1 FROM audit_log WHERE action = 'system.health_test' AND created_at > now() - interval '1 minute'")).rowCount! >= 1);

  // ---- alerts ----
  ok('alert addresses must be addresses', (await admin.put('/admin/health/settings', { alerts: true, extra_emails: ['not an address'] })).status === 400);
  r = await admin.put('/admin/health/settings', { alerts: true, extra_emails: [alertTo] });
  ok('further alert addresses are kept', r.status === 200 && r.data.extra_emails[0] === alertTo, r.data);
  // A known-good audit result, then a bad one: the audit check turns red.
  await admin.post('/audit/chain/verify', {});
  await admin.post('/admin/health/run', {});
  const saved = (await db.query("SELECT value FROM settings WHERE key = 'audit_verify_last'")).rows[0].value;
  await db.query("UPDATE settings SET value = $1 WHERE key = 'audit_verify_last'", [{ ...saved, ok: false, problems: [{ position: 1, kind: 'changed', text: `Record 1 was changed ${s}` }] }]);
  r = await admin.post('/admin/health/run', {});
  ok('a check turns red', by(r.data.last, 'audit').level === 'fail' && r.data.last.overall === 'fail' && new RegExp(s).test(by(r.data.last, 'audit').text), by(r.data.last, 'audit'));
  const mails = (await db.query("SELECT payload FROM jobs WHERE kind = 'email.send' AND payload->>'to' = $1", [alertTo])).rows;
  ok('…and the alert goes out by email', mails.length === 1 && /needs attention/.test(mails[0].payload.subject) && /Audit trail/.test(mails[0].payload.subject), mails.map((m) => m.payload.subject));
  ok('…and in NERP', (await db.query("SELECT 1 FROM notifications n JOIN users u ON u.id = n.user_id WHERE u.email = 'admin@acme.test' AND n.kind = 'system.health' AND n.created_at > now() - interval '1 minute'")).rowCount! >= 1);
  await admin.post('/admin/health/run', {});
  ok('…once, not every run while it stays red', (await db.query("SELECT 1 FROM jobs WHERE kind = 'email.send' AND payload->>'to' = $1", [alertTo])).rowCount === 1);
  await admin.put('/admin/health/settings', { alerts: false, extra_emails: [alertTo] });
  await admin.post('/audit/chain/verify', {});
  await admin.post('/admin/health/run', {});
  await db.query("UPDATE settings SET value = $1 WHERE key = 'audit_verify_last'", [{ ...saved, ok: false, problems: [{ position: 1, kind: 'changed', text: 'again' }] }]);
  await admin.post('/admin/health/run', {});
  ok('alerts can be turned off', (await db.query("SELECT 1 FROM jobs WHERE kind = 'email.send' AND payload->>'to' = $1", [alertTo])).rowCount === 1);

  const nina = new Client();
  await admin.post('/users', { email: `nina.${s}@acme.test`, password: 'password123', first_name: 'Nina', last_name: `H${s}` });
  await nina.post('/auth/login', { email: `nina.${s}@acme.test`, password: 'password123' });
  ok('health is for those who manage the system', (await nina.get('/admin/health')).status === 403 && (await nina.post('/admin/health/run', {})).status === 403);
} finally {
  await admin.put('/admin/health/settings', before ?? { alerts: true, extra_emails: [] }).catch(() => {});
  await admin.post('/audit/chain/verify', {}).catch(() => {});
  await admin.post('/admin/health/run', {}).catch(() => {});
  await db.query("DELETE FROM jobs WHERE kind = 'email.send' AND payload->>'to' = $1", [alertTo]).catch(() => {});
  await db.end();
  await cleanupRun(s);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
