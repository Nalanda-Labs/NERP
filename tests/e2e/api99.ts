// Files encrypted at rest: uploads stored encrypted under the configured key (nothing in the
// clear on disk), downloads and byte ranges decrypted, a changed file refused, files from
// before encryption encrypted by the background job (and still readable on the way), removed
// files put aside for point-in-time restores, and the Storage page's encryption status.
import { API, Client, waitFor } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { dbUrl } from '../db/cleanup.ts';
import { createHash, randomBytes } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import pg from 'pg';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 700)); };
const stamp = Date.now().toString(36);
// The key tests/e2e/tools/restart-server.sh starts the server with.
const SECRET = process.env.FILE_ENCRYPTION_KEY ?? 'nerp-test-file-key-0123456789abcdef';
const sha = (b: Buffer | string) => createHash('sha256').update(b).digest();
const keyId = sha(Buffer.concat([Buffer.from('nerp:file-key-id:v1:'), sha(`nerp:file-key:v1:${SECRET}`)])).subarray(0, 8).toString('hex');
const CHUNK = 64 * 1024;

const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const status = (await admin.get('/admin/storage')).data.encryption;
if (!status?.enabled) {
  console.log('skipped: start the server with FILE_ENCRYPTION_KEY (tests/e2e/tools/restart-server.sh does)');
  process.exit(0);
}
const email = `ann.${stamp}@acme.test`;
const annUser = (await admin.post('/users', { email, password: 'password123', first_name: 'Ann', last_name: `Enc${stamp}` })).data;
const ann = new Client();
await ann.post('/auth/login', { email, password: 'password123' });
const get = (path: string, headers: Record<string, string> = {}) => fetch(API.replace(/\/api$/, '') + path, { headers: { authorization: `Bearer ${ann.token}`, ...headers } });
const env = readFileSync(new URL('../../backend/.env', import.meta.url), 'utf8');
const uploads = join(new URL('../../backend/', import.meta.url).pathname, (/^UPLOAD_DIR=(.*)$/m.exec(env)?.[1] ?? './data/uploads').trim());
const today = new Date().toISOString().slice(0, 10);
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
const row = async (id: string) => (await db.query('SELECT storage_path, enc_key, size_bytes, sha256 FROM files WHERE id = $1', [id])).rows[0];
const upload = async (data: Buffer, name: string, mime = 'application/octet-stream') => {
  const fd = new FormData(); fd.append('file', new Blob([data], { type: mime }), name);
  return (await ann.req('POST', '/files', fd)).data;
};
const legacyDir = join(uploads, 'legacy-test');

try {
  ok('the key in use is the one configured', status.key_id === keyId, [status.key_id, keyId]);

  // ---- an upload ----
  const data = randomBytes(3 * CHUNK + 1234);
  const file = await upload(data, `secret-${stamp}.bin`);
  let r = await row(file.id);
  ok('stored under the current key', r.enc_key === keyId && Number(r.size_bytes) === data.length && r.sha256 === sha(data).toString('hex'), r);
  const disk = readFileSync(join(uploads, r.storage_path));
  ok('encrypted on disk: header, one tag per chunk', disk.subarray(0, 8).toString() === 'NERPENC1' && disk.length === 32 + data.length + 4 * 16, disk.length);
  let clear = false;
  for (let i = 0; i + 32 <= data.length; i += 4096) if (disk.includes(data.subarray(i, i + 32))) { clear = true; break; }
  ok('nothing of it in the clear', !clear);
  let res = await get(`/api/files/${file.id}/content`);
  ok('downloads decrypted', res.status === 200 && Buffer.from(await res.arrayBuffer()).equals(data) && res.headers.get('content-length') === String(data.length));
  res = await get(`/api/files/${file.id}/content`, { range: `bytes=${CHUNK - 10}-${2 * CHUNK + 5}` });
  ok('a range across chunks', res.status === 206 && Buffer.from(await res.arrayBuffer()).equals(data.subarray(CHUNK - 10, 2 * CHUNK + 6))
    && res.headers.get('content-range') === `bytes ${CHUNK - 10}-${2 * CHUNK + 5}/${data.length}`);
  res = await get(`/api/files/${file.id}/content`, { range: `bytes=-7` });
  ok('the last bytes', res.status === 206 && Buffer.from(await res.arrayBuffer()).equals(data.subarray(data.length - 7)));
  const empty = await upload(Buffer.alloc(0), `empty-${stamp}.txt`, 'text/plain');
  res = await get(`/api/files/${empty.id}/content`);
  ok('an empty file', res.status === 200 && (await res.arrayBuffer()).byteLength === 0 && (await row(empty.id)).enc_key === keyId);

  // ---- a file changed on disk ----
  const victim = await upload(Buffer.from('x'.repeat(5000)), `victim-${stamp}.txt`, 'text/plain');
  const vpath = join(uploads, (await row(victim.id)).storage_path);
  const vbytes = readFileSync(vpath);
  vbytes[100] ^= 1;
  writeFileSync(vpath, vbytes);
  res = await get(`/api/files/${victim.id}/content`);
  const refused = res.status >= 500 || await res.arrayBuffer().then((b) => b.byteLength !== 5000, () => true);
  ok('a changed file is not served', refused, res.status);

  // ---- files from before encryption ----
  mkdirSync(legacyDir, { recursive: true });
  const old = randomBytes(CHUNK * 2 + 99);
  writeFileSync(join(legacyDir, stamp), old);
  const oldId = (await db.query(`INSERT INTO files (owner_id, original_name, mime_type, size_bytes, sha256, storage_path) VALUES ($1, $2, 'application/octet-stream', $3, $4, $5) RETURNING id`,
    [annUser.id, `old-${stamp}.bin`, old.length, sha(old).toString('hex'), `legacy-test/${stamp}`])).rows[0].id;
  res = await get(`/api/files/${oldId}/content`, { range: 'bytes=10-19' });
  ok('a file from before is still served as it is', res.status === 206 && Buffer.from(await res.arrayBuffer()).equals(old.subarray(10, 20)));
  ok('…and counted as not encrypted yet', (await admin.get('/admin/storage')).data.encryption.not_encrypted >= 1);
  await db.query(`INSERT INTO jobs (kind, payload) VALUES ('files.encrypt', '{}')`);
  r = await waitFor(async () => { const x = await row(oldId); return x.enc_key ? x : null; }, 60_000);
  ok('the background job encrypts it', r?.enc_key === keyId && r.storage_path !== `legacy-test/${stamp}` && readFileSync(join(uploads, r.storage_path)).subarray(0, 8).toString() === 'NERPENC1', r);
  res = await get(`/api/files/${oldId}/content`);
  ok('…and it reads the same', res.status === 200 && Buffer.from(await res.arrayBuffer()).equals(old));
  ok('the clear copy is put aside, not left in place', !existsSync(join(legacyDir, stamp)) && existsSync(join(uploads, '.deleted', today, 'legacy-test', stamp)));

  // ---- removing a file ----
  const gone = (await row(file.id)).storage_path;
  ok('removing a file', (await ann.del(`/files/${file.id}`)).status === 200);
  const aside = join(uploads, '.deleted', today, gone);
  ok('kept aside (encrypted) for point-in-time restores', !existsSync(join(uploads, gone)) && existsSync(aside) && readFileSync(aside).subarray(0, 8).toString() === 'NERPENC1');
  const st = (await admin.get('/admin/storage')).data.encryption;
  ok('the storage page shows it', st.enabled && st.encrypted >= 2 && st.keep_deleted_days === 14 && st.deleted.files >= 2, st);
  ok('only for people who manage files', (await ann.get('/admin/storage')).status === 403);
  rmSync(aside, { force: true });
} finally {
  for (const f of (await db.query('DELETE FROM files WHERE owner_id = $1 RETURNING storage_path', [annUser.id]).catch(() => ({ rows: [] }))).rows) rmSync(join(uploads, f.storage_path), { force: true });
  rmSync(legacyDir, { recursive: true, force: true });
  rmSync(join(uploads, '.deleted', today, 'legacy-test'), { recursive: true, force: true });
  await db.end();
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
