// Videos for every browser: uploads that browsers can't all play (here an MPEG-4 Part 2 AVI and
// a WebM like the browser's recorder makes) get an H.264/AAC MP4 copy and a poster frame in the
// background; H.264 MP4s keep their original; players reach them through download links; only
// people who may read the file may read its copies; other files aren't touched.
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const B = 'http://localhost:8080';
const stamp = Date.now().toString(36);
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const waitFor = async <T>(f: () => Promise<T | null | undefined | false>, ms = 90000): Promise<T | null> => { const end = Date.now() + ms; while (Date.now() < end) { const v = await f().catch(() => null); if (v) return v as T; await sleep(700); } return null; };
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const mk = async (first: string) => {
  const email = `${first.toLowerCase()}.${stamp}@acme.test`;
  const u = (await admin.post('/users', { email, password: 'password123', first_name: first, last_name: `Vd${stamp}` })).data;
  const c = new Client();
  await c.post('/auth/login', { email, password: 'password123' });
  c.id = u.id;
  return c;
};
const dir = mkdtempSync(join(tmpdir(), 'nerp-video-'));
const make = (name: string, args: string[]) => {
  const out = join(dir, name);
  execFileSync('ffmpeg', ['-hide_banner', '-loglevel', 'error', '-y', '-f', 'lavfi', '-i', 'testsrc=duration=2:size=320x240:rate=15', '-f', 'lavfi', '-i', 'sine=frequency=440:duration=2', ...args, out]);
  return readFileSync(out);
};
const probe = (bytes: Uint8Array) => {
  const p = join(dir, `probe-${Math.random()}`);
  writeFileSync(p, bytes);
  return JSON.parse(execFileSync('ffprobe', ['-v', 'error', '-print_format', 'json', '-show_streams', '-show_format', p]).toString());
};
const upload = async (c: Client, name: string, bytes: Uint8Array, type: string) => { const fd = new FormData(); fd.append('file', new Blob([new Uint8Array(bytes)], { type }), name); return (await c.req('POST', '/files', fd)).data.id as string; };
const status = (c: Client, id: string) => c.get(`/files/${id}/video-status`);

try {
  const ann = await mk('Ann'), bob = await mk('Bob');
  const avi = make('clip.avi', ['-c:v', 'mpeg4', '-c:a', 'mp3']);
  const webm = make('clip.webm', ['-c:v', 'libvpx', '-c:a', 'libopus', '-b:v', '200k']);
  const mp4 = make('clip.mp4', ['-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-c:a', 'aac', '-movflags', '+faststart']);

  const aviId = await upload(ann, 'Site visit.avi', avi, 'video/x-msvideo');
  const st = (await status(ann, aviId)).data;
  ok('an uploaded video is queued for a web copy', ['pending', 'converting', 'ready'].includes(st.status), st);
  const done = await waitFor(async () => { const s = (await status(ann, aviId)).data; return s.status === 'ready' ? s : null; });
  ok('…which is made', !!done && done.web && done.poster && Math.round(done.duration_secs) === 2 && done.width === 320, done);
  const v = await ann.req('GET', `/files/${aviId}/video`, undefined, true) as Response;
  const info = probe(new Uint8Array(await v.arrayBuffer()));
  const vs = info.streams.find((s: any) => s.codec_type === 'video'), as = info.streams.find((s: any) => s.codec_type === 'audio');
  ok('the video played is H.264 with AAC sound, in MP4', v.status === 200 && vs?.codec_name === 'h264' && as?.codec_name === 'aac' && info.format.format_name.includes('mp4'), [vs?.codec_name, as?.codec_name, info.format.format_name]);
  const p = await ann.req('GET', `/files/${aviId}/poster`, undefined, true) as Response;
  ok('a poster frame', p.status === 200 && p.headers.get('content-type')?.startsWith('image/jpeg'));
  const orig = await ann.req('GET', `/files/${aviId}/content`, undefined, true) as Response;
  ok('the original stays as it was', Buffer.from(await orig.arrayBuffer()).equals(avi));

  const webmId = await upload(ann, 'Video announcement.webm', webm, 'video/webm');
  const w = await waitFor(async () => { const s = (await status(ann, webmId)).data; return s.status === 'ready' ? s : null; });
  ok('a recording from the browser (WebM) gets an MP4 copy too', !!w, w);

  const mp4Id = await upload(ann, 'Ready.mp4', mp4, 'video/mp4');
  const m = await waitFor(async () => { const s = (await status(ann, mp4Id)).data; return s.status === 'original' ? s : null; });
  ok('an H.264 MP4 plays as it is (no copy)', !!m && !m.web && m.poster, m);
  const mv = await ann.req('GET', `/files/${mp4Id}/video`, undefined, true) as Response;
  ok('…and is served as its original', Buffer.from(await mv.arrayBuffer()).equals(mp4));

  // Players get a download link (a <video> can't send the sign-in header).
  const link = (await ann.post('/download-links', { path: `/api/files/${aviId}/video` })).data.url;
  const viaLink = await fetch(B + link, { headers: { range: 'bytes=0-99' } });
  ok('players reach it through a download link, by ranges', viaLink.status === 206 && (await viaLink.arrayBuffer()).byteLength === 100, viaLink.status);
  const posterLink = (await ann.post('/download-links', { path: `/api/files/${aviId}/poster` })).data.url;
  ok('…and the poster', (await fetch(B + posterLink)).status === 200);
  ok('a link for one file\'s video opens no other', (await fetch(B + link.replace(aviId, webmId))).status === 401);

  ok('people who may not read the file get nothing', (await bob.get(`/files/${aviId}/video-status`)).status === 404 && (await bob.req('GET', `/files/${aviId}/video`, undefined, true) as Response).status === 404);
  ok('…nor do signed-out people', (await fetch(`${B}/api/files/${aviId}/poster`)).status === 401);

  // Shared in a post: readers play it.
  await ann.post('/feed', { body: `Watch this ${stamp}`, to_all: false, users: [bob.id], file_ids: [aviId] });
  ok('readers of a post with the video play it', (await bob.get(`/files/${aviId}/video-status`)).data.status === 'ready');

  const txt = await upload(ann, 'notes.txt', new TextEncoder().encode('hi'), 'text/plain');
  ok('other files aren\'t touched', (await status(ann, txt)).data.status === 'none');
} finally {
  rmSync(dir, { recursive: true, force: true });
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
