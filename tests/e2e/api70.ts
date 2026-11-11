// Backups from the app: back up now (the deploy script, into BACKUP_DIR, which the dev
// server sets to the test output folder), the list with each backup's manifest, downloading
// a database dump, checking the newest restores, and who may do it.
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 1500)); };
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const email = `bea.${stamp}@acme.test`;
await admin.post('/users', { email, password: 'password123', first_name: 'Bea', last_name: `Bak${stamp}` });
const bea = new Client();
await bea.post('/auth/login', { email, password: 'password123' });
const settle = async () => {
  for (let i = 0; i < 600; i++) {
    const r = await admin.get('/admin/backups');
    if (!r.data.running) return r.data;
    await sleep(500);
  }
  throw new Error('still running');
};

try {
  ok('backups are for system administrators', (await bea.get('/admin/backups')).status === 403 && (await bea.post('/admin/backups')).status === 403);
  let r = await admin.get('/admin/backups');
  ok('the scripts are found', r.status === 200 && r.data.scripts === true, r.data);
  const before = r.data.items.length;
  r = await admin.post('/admin/backups');
  ok('a backup starts', r.status === 200 && r.data.started, r.data);
  ok('one at a time', (await admin.post('/admin/backups')).status === 409);
  let info = await settle();
  ok('it succeeded', info.last_backup?.ok === true, JSON.stringify(info.last_backup?.output));
  const newest = info.items[0];
  ok('and is listed with its manifest', info.items.length === before + 1 && newest.has_dump && newest.manifest.database_bytes > 0 && newest.manifest.last_migration >= 67, info.items[0]);
  ok('with the command to restore it', info.restore_command?.includes(`nerp-restore.sh ${info.dir}/${newest.name} --yes`), info.restore_command);
  const res = await admin.req('GET', `/admin/backups/${newest.name}/database.dump`, undefined, true);
  const head = Buffer.from(await res.arrayBuffer()).subarray(0, 5).toString();
  ok('its database can be downloaded', res.status === 200 && head === 'PGDMP' && /attachment/.test(res.headers.get('content-disposition') ?? ''), [res.status, head]);
  ok('only real backup names', (await admin.req('GET', '/admin/backups/..%2F..%2Fetc/database.dump', undefined, true)).status === 404);
  ok('downloads are for administrators too', (await bea.req('GET', `/admin/backups/${newest.name}/database.dump`, undefined, true)).status === 403);
  r = await admin.post('/admin/backups/verify');
  ok('a restore check starts', r.status === 200, r.data);
  info = await settle();
  ok('the newest backup restores', info.last_verify?.ok === true, info.last_verify?.output);
} finally {
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
