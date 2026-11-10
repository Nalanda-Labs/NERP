// Streaming files: byte ranges (every form, and ranges outside the file), download links (made
// for one path, usable without the token, for GET only, ending with the session or the account,
// still checking who may read the file), drive and public-link downloads with ranges (a
// download counted once), and a 400 MB file sent without the server holding it in memory.
import { API, Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { dbUrl } from '../db/cleanup.ts';
import { createHash, randomBytes } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { mkdirSync, openSync, writeSync, closeSync, readFileSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import pg from 'pg';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 700)); };
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const user = async (first: string) => {
  const u = (await admin.post('/users', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123', first_name: first, last_name: 'Stream' })).data;
  const c = new Client();
  await c.post('/auth/login', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123' });
  c.id = u.id;
  return c;
};
const ann = await user('Ann');
const bob = await user('Bob');
const get = (path: string, headers: Record<string, string> = {}, c: Client | null = ann) =>
  fetch(API.replace(/\/api$/, '') + path, { headers: { ...(c?.token ? { authorization: `Bearer ${c.token}` } : {}), ...headers } });
const bytesOf = async (r: Response) => Buffer.from(await r.arrayBuffer());
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();

try {
  // ---- ranges ----
  const data = randomBytes(3 * 1024 * 1024 + 17);
  const fd = new FormData(); fd.append('file', new Blob([data], { type: 'video/mp4' }), `clip-${stamp}.mp4`);
  const file = (await ann.req('POST', '/files', fd)).data;
  const path = `/api/files/${file.id}/content`;
  let r = await get(path);
  ok('the whole file, saying ranges are welcome', r.status === 200 && r.headers.get('accept-ranges') === 'bytes' && Number(r.headers.get('content-length')) === data.length && (await bytesOf(r)).equals(data));
  ok('video shows in the page', /^inline/.test(get.length ? (await get(path)).headers.get('content-disposition') ?? '' : ''));
  r = await get(path, { range: 'bytes=0-99' });
  ok('a range: 206, the right bytes', r.status === 206 && r.headers.get('content-range') === `bytes 0-99/${data.length}` && (await bytesOf(r)).equals(data.subarray(0, 100)));
  r = await get(path, { range: 'bytes=3000000-' });
  ok('from a byte to the end', r.status === 206 && (await bytesOf(r)).equals(data.subarray(3000000)));
  r = await get(path, { range: 'bytes=-50' });
  ok('the last bytes', r.status === 206 && (await bytesOf(r)).equals(data.subarray(data.length - 50)));
  r = await get(path, { range: `bytes=100-${data.length + 5000}` });
  ok('an end past the file is cut to it', r.status === 206 && (await bytesOf(r)).length === data.length - 100);
  r = await get(path, { range: `bytes=${data.length + 1}-` });
  ok('a start past the file: 416', r.status === 416 && r.headers.get('content-range') === `bytes */${data.length}`);
  ok('backwards: 416', (await get(path, { range: 'bytes=500-100' })).status === 416);
  ok('several ranges: the whole file', (await get(path, { range: 'bytes=0-1,5-9' })).status === 200);
  ok('nonsense is ignored', (await get(path, { range: 'pages=1-2' })).status === 200);

  // ---- download links ----
  ok('only for file downloads', (await ann.post('/download-links', { path: `/api/files/${file.id}` })).status === 400
    && (await ann.post('/download-links', { path: '/api/users' })).status === 400 && (await ann.post('/download-links', { path: `${path}?dl=x` })).status === 400);
  let link = (await ann.post('/download-links', { path })).data.url as string;
  ok('a link', link.startsWith(`${path}?dl=`));
  r = await get(link, {}, null);
  ok('works without the token', r.status === 200 && (await bytesOf(r)).equals(data));
  r = await get(`${link}&name=${encodeURIComponent('My clip · 1.mp4')}`, {}, null);
  ok('saved under the name the page asks for', (r.headers.get('content-disposition') ?? '').includes(`filename*=UTF-8''My%20clip%20%C2%B7%201.mp4`), r.headers.get('content-disposition'));
  r = await get(`${path}?name=${encodeURIComponent('../../etc/\npasswd')}`);
  ok('a name loses slashes and control characters', /filename\*=UTF-8''\.\.\.\.etcpasswd/.test(r.headers.get('content-disposition') ?? ''), r.headers.get('content-disposition'));
  r = await get(link, { range: 'bytes=10-19' }, null);
  ok('with ranges, as a video player asks', r.status === 206 && (await bytesOf(r)).equals(data.subarray(10, 20)));
  ok('and again (a player asks many times)', (await get(link, { range: 'bytes=20-29' }, null)).status === 206);
  const other = await user('Cat');
  const theirs = (await other.req('POST', '/files', (() => { const f = new FormData(); f.append('file', new Blob(['secret']), `secret-${stamp}.txt`); return f; })())).data;
  const token = link.split('dl=')[1];
  ok('not for another file', (await get(`/api/files/${theirs.id}/content?dl=${token}`, {}, null)).status === 401);
  ok('not for another path', (await get(`/api/files/${file.id}?dl=${token}`, {}, null)).status === 401);
  ok('GET only', [401, 405].includes((await fetch(API.replace(/\/api$/, '') + link, { method: 'DELETE' })).status));
  ok('a made-up token', (await get(`${path}?dl=nope`, {}, null)).status === 401);
  const bobsLink = (await bob.post('/download-links', { path: `/api/files/${theirs.id}/content` })).data.url;
  ok('a link doesn\'t give access to what its maker can\'t read', (await get(bobsLink, {}, null)).status === 404);
  await ann.post('/auth/logout');
  ok('signing out ends the link', (await get(link, {}, null)).status === 401);
  await ann.post('/auth/login', { email: `ann.${stamp}@acme.test`, password: 'password123' });
  link = (await ann.post('/download-links', { path })).data.url;
  await admin.patch(`/users/${ann.id}`, { is_active: false });
  ok('so does deactivation', (await get(link, {}, null)).status === 401);
  await admin.patch(`/users/${ann.id}`, { is_active: true });
  await ann.post('/auth/login', { email: `ann.${stamp}@acme.test`, password: 'password123' });

  // ---- drive ----
  const node = (await ann.post('/drive/files', { file_id: file.id, name: `clip-${stamp}.mp4` })).data;
  r = await get(`/api/drive/nodes/${node.id}/download`, { range: 'bytes=5-9' });
  ok('drive downloads take ranges', r.status === 206 && (await bytesOf(r)).equals(data.subarray(5, 10)));
  const dl = (await ann.post('/download-links', { path: `/api/drive/nodes/${node.id}/download` })).data.url;
  ok('and download links', (await get(dl, {}, null)).status === 200);
  const pub = (await ann.post(`/drive/nodes/${node.id}/links`, {})).data;
  const pubPath = `/api/public/drive/${pub.token ?? pub.url?.split('/').pop()}/download`;
  await get(pubPath, { range: 'bytes=0-' }, null);
  await get(pubPath, { range: 'bytes=1000-1999' }, null);
  await get(pubPath, { range: 'bytes=2000-2999' }, null);
  r = await get(pubPath, { range: 'bytes=10-20' }, null);
  ok('public links take ranges', r.status === 206 && (await bytesOf(r)).equals(data.subarray(10, 21)));
  const downloads = (await db.query('SELECT downloads FROM drive_links WHERE node_id = $1', [node.id])).rows[0]?.downloads;
  ok('a download is counted once, not per piece', downloads === 1, downloads);

  // ---- a big file, without the server holding it ----
  const pid = execFileSync('pgrep', ['-x', 'nerp']).toString().trim().split('\n')[0];
  const rss = () => Number(/VmRSS:\s+(\d+)/.exec(readFileSync(`/proc/${pid}/status`, 'utf8'))![1]) / 1024;
  const env = readFileSync(new URL('../../backend/.env', import.meta.url), 'utf8');
  const uploads = join(new URL('../../backend/', import.meta.url).pathname, (/^UPLOAD_DIR=(.*)$/m.exec(env)?.[1] ?? './data/uploads').trim());
  const rel = `stream-test/${stamp}`;
  mkdirSync(join(uploads, 'stream-test'), { recursive: true });
  const big = join(uploads, rel);
  const SIZE = 400 * 1024 * 1024;
  const chunk = randomBytes(1024 * 1024);
  const h = createHash('sha256');
  const fh = openSync(big, 'w');
  for (let i = 0; i < SIZE / chunk.length; i++) { writeSync(fh, chunk); h.update(chunk); }
  closeSync(fh);
  const bigId = (await db.query(`INSERT INTO files (owner_id, original_name, mime_type, size_bytes, sha256, storage_path) VALUES ($1, $2, 'video/mp4', $3, $4, $5) RETURNING id`,
    [ann.id, `big-${stamp}.mp4`, SIZE, h.digest('hex'), rel])).rows[0].id;
  const before = rss();
  let peak = before;
  const timer = setInterval(() => { peak = Math.max(peak, rss()); }, 50);
  const res = await get(`/api/files/${bigId}/content`);
  const reader = res.body!.getReader();
  const got = createHash('sha256');
  let total = 0;
  for (;;) { const { done, value } = await reader.read(); if (done) break; total += value.length; got.update(value); await new Promise((r) => setImmediate(r)); }
  clearInterval(timer);
  ok('400 MB arrive whole', total === SIZE && got.digest('hex') === (await db.query('SELECT sha256 FROM files WHERE id = $1', [bigId])).rows[0].sha256);
  ok('without the server holding it in memory', peak - before < 100, `${Math.round(before)} MB before, ${Math.round(peak)} MB at most`);
  console.log(`      (server memory: ${Math.round(before)} MB before, ${Math.round(peak)} MB at most while sending ${SIZE >> 20} MB)`);
  await db.query('DELETE FROM files WHERE id = $1', [bigId]);
  rmSync(big, { force: true });
} finally {
  await db.end();
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
