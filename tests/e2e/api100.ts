// Point-in-time restore with the deploy scripts, against a throwaway PostgreSQL server whose
// WAL nerp-wal-receive.sh streams over the network (pg_receivewal, a replication slot; nothing
// set up on the server for it): nerp-backup.sh takes a base backup, changes
// go on, and nerp-pitr-restore.sh puts the database back as it was at a chosen moment — into
// a copy, then over the "live" database (with a file removed since then put back). Also the
// Backups page's receiving status and its checks before restoring to a moment from the app.
// Needs PostgreSQL's server programs (PG_BIN, ~/.local/pg18/usr/bin or
// /usr/lib/postgresql/18/bin); skipped without them.
import { Client } from './lib.ts';
import { OUTPUT } from './output.ts';
import { execFileSync, spawn, spawnSync, type ChildProcess } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 1500)); };
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const scripts = new URL('../../deploy/backup/', import.meta.url).pathname;

// ---- the app's side ----
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const page = (await admin.get('/admin/backups')).data;
ok('the Backups page shows whether WAL is received', typeof page.pitr?.receiving === 'boolean' && typeof page.pitr.set_up === 'boolean' && page.pitr.slot === 'nerp_wal' && page.pitr.command.includes('nerp-pitr-restore.sh --dest') && page.pitr.command.includes('--into-live --yes'), page.pitr);
let r = await admin.post('/admin/backups/restore-to-moment', { to: new Date(Date.now() + 3600e3).toISOString(), password: 'supersecret1', confirm: 'restore' });
ok('restoring to a moment: not one in the future', r.status === 400 && /past/.test(r.data.error?.message), r.data);
if (!page.pitr.earliest) {
  r = await admin.post('/admin/backups/restore-to-moment', { to: new Date(Date.now() - 60e3).toISOString(), password: 'supersecret1', confirm: 'restore' });
  ok('…nor without base backups', r.status === 400 && /no base backups/.test(r.data.error?.message), r.data);
}
const ann = new Client();
ok('only for people who manage the system', (await ann.post('/admin/backups/restore-to-moment', { to: '2026-01-01T00:00:00Z', password: 'x', confirm: 'restore' })).status === 401);

// ---- the scripts, with a throwaway server ----
const bin = [process.env.PG_BIN, join(homedir(), '.local/pg18/usr/bin'), '/usr/lib/postgresql/18/bin'].find((d) => d && existsSync(join(d, 'initdb')) && existsSync(join(d, 'pg_basebackup')));
if (!bin) {
  console.log('skipped: PostgreSQL server programs not found (set PG_BIN)');
  console.log(`\n${pass} passed, ${fail} failed`);
  process.exit(fail ? 1 : 0);
}
const root = join(OUTPUT, `pitr-${Date.now().toString(36)}`);
const data = join(root, 'cluster'), sock = join(root, 'sock'), dest = join(root, 'backups'), wal = join(dest, 'wal'), uploads = join(root, 'uploads');
mkdirSync(sock, { recursive: true });
mkdirSync(uploads, { recursive: true });
const PORT = 55440;
const env = { ...process.env, PG_BIN: bin, PGCONNECT_TIMEOUT: '5' };
const run = (cmd: string, args: string[], extra: Record<string, string> = {}) => {
  const p = spawnSync(cmd, args, { env: { ...env, ...extra }, encoding: 'utf8', timeout: 300_000 });
  return { code: p.status, out: `${p.stdout ?? ''}${p.stderr ?? ''}` };
};
const url = (db: string) => `postgres://nerp@127.0.0.1:${PORT}/${db}`;
const sql = (db: string, q: string) => execFileSync('psql', [url(db), '-Atc', q], { env, encoding: 'utf8' }).trim();
let started = false;
let receiver: ChildProcess | null = null;
try {
  execFileSync(join(bin, 'initdb'), ['-D', data, '-U', 'nerp', '--auth=trust'], { env, stdio: 'ignore' });
  writeFileSync(join(data, 'postgresql.auto.conf'), [
    `listen_addresses = '127.0.0.1'`, `port = ${PORT}`, `unix_socket_directories = '${sock}'`,
  ].join('\n') + '\n');
  writeFileSync(join(data, 'pg_hba.conf'), 'local all all trust\nhost all all 127.0.0.1/32 trust\nhost replication all 127.0.0.1/32 trust\n');
  execFileSync(join(bin, 'pg_ctl'), ['-D', data, '-l', join(root, 'server.log'), '-w', 'start'], { env, stdio: 'ignore' });
  started = true;
  sql('postgres', 'CREATE DATABASE nerp');
  sql('nerp', "CREATE TABLE _sqlx_migrations (version bigint, description text); INSERT INTO _sqlx_migrations VALUES (1, 'test'); CREATE TABLE notes (body text, at timestamptz DEFAULT clock_timestamp()); INSERT INTO notes (body) VALUES ('kept from the start')");
  const envFile = join(root, '.env');
  writeFileSync(envFile, `DATABASE_URL=${url('nerp')}\nUPLOAD_DIR=${uploads}\n`);

  // Receiving: from here on, every change streams into the archive folder.
  let said = '';
  receiver = spawn('bash', [`${scripts}nerp-wal-receive.sh`, '--env', envFile, '--dest', dest], { env, stdio: ['ignore', 'pipe', 'pipe'] });
  receiver.stdout!.on('data', (d) => (said += d));
  receiver.stderr!.on('data', (d) => (said += d));
  let active = false;
  for (let i = 0; i < 40 && !active; i++) { await sleep(500); active = sql('nerp', "SELECT count(*) FROM pg_replication_slots WHERE slot_name = 'nerp_wal' AND active") === '1'; }
  ok('receiving through its own replication slot', active && /receiving WAL into/.test(said), said);
  const second = spawnSync('bash', [`${scripts}nerp-wal-receive.sh`, '--env', envFile, '--dest', dest], { env, encoding: 'utf8', timeout: 20_000 });
  ok('…once', second.status === 1 && /another nerp-wal-receive.sh is running/.test(second.stderr), second.stderr);

  let b = run('bash', [`${scripts}nerp-backup.sh`, '--env', envFile, '--dest', dest]);
  const folder = b.out.match(/backup written to (\S+)/)?.[1] ?? '';
  const manifest = folder && existsSync(join(folder, 'manifest.txt')) ? readFileSync(join(folder, 'manifest.txt'), 'utf8') : '';
  ok('a backup with a base backup, as WAL is received', b.code === 0 && existsSync(join(folder, 'base/base.tar.gz')) && /^base_backup=1$/m.test(manifest) && /^base_wal=[0-9A-F]{24}$/m.test(manifest), b.out);

  sql('nerp', "INSERT INTO notes (body) VALUES ('written after the backup')");
  await sleep(1500);
  const moment = sql('nerp', "SELECT to_char(clock_timestamp() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS.US') || '+00'");
  await sleep(1500);
  // The mistake to undo: something written after the moment, and the first note deleted.
  sql('nerp', "INSERT INTO notes (body) VALUES ('written after the moment'); DELETE FROM notes WHERE body = 'kept from the start'");
  const day = moment.slice(0, 10);
  mkdirSync(join(uploads, '.deleted', day, '2026/11'), { recursive: true });
  writeFileSync(join(uploads, '.deleted', day, '2026/11/removed-file'), 'removed after the moment');
  const walFiles = execFileSync('ls', [wal], { encoding: 'utf8' }).split('\n');
  ok('finished segments compressed by the backup, the current one being written', walFiles.some((f) => /^[0-9A-F]{24}\.gz$/.test(f)) && walFiles.some((f) => /^[0-9A-F]{24}\.partial$/.test(f)), walFiles);

  // Into a copy first.
  sql('postgres', 'CREATE DATABASE nerp_copy');
  let p = run('bash', [`${scripts}nerp-pitr-restore.sh`, '--env', envFile, '--dest', dest, '--to', moment, '--target-db', url('nerp_copy'), '--port', String(PORT + 1)]);
  const copy = p.code === 0 ? sql('nerp_copy', 'SELECT string_agg(body, \'|\' ORDER BY at) FROM notes') : '';
  ok('restored to the moment, into a copy', copy === 'kept from the start|written after the backup', { copy, out: p.out });
  ok('the live database is untouched by that', sql('nerp', "SELECT string_agg(body, '|' ORDER BY at) FROM notes") === 'written after the backup|written after the moment');
  ok('the temporary server is gone', !readFileSync(join(root, 'server.log'), 'utf8').includes('FATAL') && !execFileSync('ls', ['-a', dest], { encoding: 'utf8' }).includes('.pitr-'));

  p = run('bash', [`${scripts}nerp-pitr-restore.sh`, '--env', envFile, '--dest', dest, '--to', moment, '--into-live']);
  ok('over the live database only with --yes', p.code === 1 && /--yes/.test(p.out), p.out);
  p = run('bash', [`${scripts}nerp-pitr-restore.sh`, '--env', envFile, '--dest', dest, '--to', moment, '--into-live', '--yes', '--port', String(PORT + 1)]);
  const live = p.code === 0 ? sql('nerp', "SELECT string_agg(body, '|' ORDER BY at) FROM notes") : '';
  ok('restored over the live database', live === 'kept from the start|written after the backup', { live, out: p.out });
  ok('files removed since then are back', readFileSync(join(uploads, '2026/11/removed-file'), 'utf8') === 'removed after the moment');

  p = run('bash', [`${scripts}nerp-pitr-restore.sh`, '--env', envFile, '--dest', dest, '--to', '2001-01-01 00:00:00+00', '--target-db', url('nerp_copy')]);
  ok('a moment before the oldest base backup: said plainly', p.code === 1 && /no base backup from before/.test(p.out), p.out);
  p = run('bash', [`${scripts}nerp-pitr-restore.sh`, '--env', envFile, '--dest', dest, '--to', 'yesterday-ish', '--target-db', url('nerp_copy')]);
  ok('a moment that can\'t be read', p.code === 2 && /can't read the moment/.test(p.out), p.out);

  // A second backup: archived WAL from before the oldest base backup kept goes.
  b = run('bash', [`${scripts}nerp-backup.sh`, '--env', envFile, '--dest', dest, '--keep-days', '14']);
  ok('a second backup', b.code === 0, b.out);
} finally {
  receiver?.kill();
  if (started) spawnSync(join(bin, 'pg_ctl'), ['-D', data, '-m', 'immediate', '-w', 'stop'], { env, stdio: 'ignore' });
  rmSync(root, { recursive: true, force: true });
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
