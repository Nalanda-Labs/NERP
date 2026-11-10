// A throwaway LiveKit media server for the call tests (the installed livekit-server, run as
// the current user on 127.0.0.1:7890 with a test key), and a stand-in for LiveKit Egress's
// API on 127.0.0.1:7899: it checks that requests are signed with the API secret and allowed
// to record, and "records" by writing a file where Egress would. NERP calls the server API
// through it (LIVEKIT_API_URL); everything else goes to the real server.
//
// NERP must run with LIVEKIT_ENV below.
import { spawn, type ChildProcess } from 'node:child_process';
import { createHmac } from 'node:crypto';
import { existsSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { createServer, type Server } from 'node:http';
import { join } from 'node:path';
import { OUTPUT } from './output.ts';

export const LIVEKIT_PORT = 7890;
export const EGRESS_API_PORT = 7899;
export const LIVEKIT_KEY = 'nerp-test-key';
export const LIVEKIT_SECRET = 'nerp-test-secret-0123456789abcdefghijkl';
export const RECORDINGS_DIR = join(OUTPUT, 'livekit-recordings');
export const LIVEKIT_ENV = {
  LIVEKIT_URL: `ws://127.0.0.1:${LIVEKIT_PORT}`,
  LIVEKIT_API_URL: `http://127.0.0.1:${EGRESS_API_PORT}`,
  LIVEKIT_API_KEY: LIVEKIT_KEY,
  LIVEKIT_API_SECRET: LIVEKIT_SECRET,
  LIVEKIT_RECORDINGS_DIR: RECORDINGS_DIR,
};
export const LIVEKIT_SERVER = ['/usr/bin/livekit-server', '/usr/local/bin/livekit-server'].find((p) => existsSync(p));

export async function startLiveKit(): Promise<() => void> {
  const proc: ChildProcess = spawn(LIVEKIT_SERVER!, [
    '--bind', '127.0.0.1', '--node-ip', '127.0.0.1',
    '--config-body', [
      `port: ${LIVEKIT_PORT}`,
      'rtc:',
      `  tcp_port: ${LIVEKIT_PORT + 1}`,
      '  port_range_start: 50100',
      '  port_range_end: 50200',
      '  use_external_ip: false',
      'keys:',
      `  ${LIVEKIT_KEY}: ${LIVEKIT_SECRET}`,
      'logging:',
      '  level: warn',
    ].join('\n'),
  ], { stdio: 'ignore' });
  proc.unref();
  for (let i = 0; i < 100; i++) {
    if (proc.exitCode !== null) throw new Error('livekit-server did not start');
    try { if ((await fetch(`http://127.0.0.1:${LIVEKIT_PORT}/`)).ok) return () => proc.kill('SIGTERM'); } catch { /* not yet */ }
    await new Promise((r) => setTimeout(r, 100));
  }
  proc.kill('SIGTERM');
  throw new Error('livekit-server did not answer');
}

type Egress = { egressId: string; roomName: string; filepath: string; status: string; error?: string; fileResults?: any[]; startedAt: number };

export interface FakeEgress {
  egresses: Map<string, Egress>;
  /** Requests refused because of a bad signature or missing permission. */
  refused: number;
  /** Answer the next start like LiveKit does when no Egress worker is running. */
  unavailable: boolean;
  /** The next recording fails while being finished. */
  failNext: boolean;
  /** How the next stopped recording ends: complete with a file (default), complete but the
   *  file never appears, or stopped by Egress's time limit (with or without a file). */
  nextEnd: 'complete' | 'complete-no-file' | 'limit-with-file' | 'limit-no-file';
  /** Every request fails as if Egress's API were down. */
  down: boolean;
  /** Starting takes this long (to test two starts at once). */
  startDelayMs: number;
  close(): Promise<void>;
}

function verify(auth: string | undefined): any {
  const [h, p, sig] = (auth ?? '').replace(/^Bearer /, '').split('.');
  if (!h || !p || !sig) return null;
  const want = createHmac('sha256', LIVEKIT_SECRET).update(`${h}.${p}`).digest('base64url');
  if (want !== sig) return null;
  const claims = JSON.parse(Buffer.from(p, 'base64url').toString());
  if (claims.iss !== LIVEKIT_KEY || claims.exp < Date.now() / 1000) return null;
  return claims;
}

export async function startFakeEgress(): Promise<FakeEgress> {
  mkdirSync(RECORDINGS_DIR, { recursive: true });
  let server: Server;
  const fake: FakeEgress = { egresses: new Map(), refused: 0, unavailable: false, failNext: false, nextEnd: 'complete', down: false, startDelayMs: 0, close: () => new Promise((r) => server.close(() => r())) };
  let n = 0;
  server = createServer(async (req, res) => {
    const json = (status: number, body: unknown) => { res.writeHead(status, { 'content-type': 'application/json' }); res.end(JSON.stringify(body)); };
    // NERP's reachability check: the real server's answer.
    if (req.method === 'GET' && req.url === '/') {
      try { const r = await fetch(`http://127.0.0.1:${LIVEKIT_PORT}/`); res.writeHead(r.status); return res.end(await r.text()); } catch { res.writeHead(502); return res.end(); }
    }
    let raw = '';
    for await (const c of req) raw += c;
    if (fake.down) return json(503, { code: 'unavailable', msg: 'service unavailable' });
    const claims = verify(req.headers.authorization);
    if (!claims?.video?.roomRecord) { fake.refused++; return json(401, { code: 'unauthenticated', msg: 'permissions denied' }); }
    const body = raw ? JSON.parse(raw) : {};
    const m = req.url?.match(/^\/twirp\/livekit\.Egress\/(\w+)$/)?.[1];
    if (m === 'StartRoomCompositeEgress') {
      if (fake.unavailable) { fake.unavailable = false; return json(503, { code: 'unavailable', msg: 'egress not connected (redis required)' }); }
      const filepath = body.file_outputs?.[0]?.filepath;
      if (!body.room_name || !filepath || body.file_outputs[0].file_type !== 'MP4') return json(400, { code: 'invalid_argument', msg: 'bad request' });
      if (fake.startDelayMs) await new Promise((r) => setTimeout(r, fake.startDelayMs));
      const e: Egress = { egressId: `EG_fake${++n}`, roomName: body.room_name, filepath, status: 'EGRESS_ACTIVE', startedAt: Date.now() };
      fake.egresses.set(e.egressId, e);
      return json(200, e);
    }
    if (m === 'StopEgress') {
      const e = fake.egresses.get(body.egress_id);
      if (!e) return json(404, { code: 'not_found', msg: 'egress not found' });
      if (e.status !== 'EGRESS_ACTIVE') return json(412, { code: 'failed_precondition', msg: `egress with status ${e.status} cannot be stopped` });
      e.status = 'EGRESS_ENDING';
      const failing = fake.failNext;
      fake.failNext = false;
      const end = fake.nextEnd;
      fake.nextEnd = 'complete';
      // Egress takes a moment to finish the file.
      setTimeout(() => {
        if (failing) { e.status = 'EGRESS_FAILED'; e.error = 'pipeline error: encoder stopped'; return; }
        const bytes = Buffer.alloc(200_000, 7);
        if (end !== 'complete-no-file' && end !== 'limit-no-file') writeFileSync(e.filepath, bytes);
        e.status = end.startsWith('limit') ? 'EGRESS_LIMIT_REACHED' : 'EGRESS_COMPLETE';
        e.fileResults = [{ filename: e.filepath, location: e.filepath, size: String(bytes.length), duration: String((Date.now() - e.startedAt) * 1_000_000 + 65_000_000_000) }];
      }, 1000);
      return json(200, e);
    }
    if (m === 'ListEgress') {
      const e = fake.egresses.get(body.egress_id);
      return json(200, { items: e ? [e] : [] });
    }
    json(404, { code: 'bad_route', msg: 'no handler' });
  });
  await new Promise<void>((r) => server.listen(EGRESS_API_PORT, '127.0.0.1', () => r()));
  return fake;
}

export function clearRecordings() {
  rmSync(RECORDINGS_DIR, { recursive: true, force: true });
}
