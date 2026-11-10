// Recording when things go wrong (LiveKit, with the stand-in Egress API): several people
// pressing Record at once, Egress finishing without a file, Egress's time limit, Egress losing a
// recording, Egress's API down for a while, calls without a chat, and who may download a
// recording (and stops being able to).
import { Client, waitFor } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { dbUrl } from '../db/cleanup.ts';
import { LIVEKIT_ENV, LIVEKIT_SERVER, clearRecordings, startFakeEgress, startLiveKit } from './livekit-server.ts';
import pg from 'pg';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 700)); };
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const user = async (first: string) => {
  const u = (await admin.post('/users', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123', first_name: first, last_name: 'Rec' })).data;
  const c = new Client();
  await c.post('/auth/login', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123' });
  c.id = u.id;
  return c;
};
if (!LIVEKIT_SERVER) { console.log('skipped: livekit-server is not installed'); process.exit(0); }
const ann = await user('Ann');
const bob = await user('Bob');
const cat = await user('Cat');
const dan = await user('Dan');
const stopServer = await startLiveKit();
const egress = await startFakeEgress();
await new Promise((r) => setTimeout(r, 5500));
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
const messages = async (c: Client, chat: string) => (await c.get(`/im/chats/${chat}/messages?limit=200`)).data.items as any[];
const recordings = async (call: string) => (await db.query('SELECT status, error, file_id FROM call_recordings WHERE call_id = $1 ORDER BY started_at', [call])).rows;
// Like browsers, keep sending heartbeats while "in" a call (NERP drops silent ones after 45 s).
const beats: ReturnType<typeof setInterval>[] = [];
const keepAlive = (c: Client, call: string, peer: string) => beats.push(setInterval(() => c.post(`/calls/${call}/heartbeat`, { peer_id: peer }).catch(() => {}), 10_000));
/** A call from a group chat with everyone joined. */
const setup = async (people: Client[]) => {
  const chat = (await ann.post('/im/chats', { title: `Rec ${stamp}`, members: people.filter((p) => p !== ann).map((p) => p.id) })).data.id;
  const call = (await ann.post('/calls', { chat_id: chat })).data;
  const peers: Record<string, string> = {};
  for (const p of people) {
    peers[p.id] = (await p.post(`/calls/${call.id}/join`)).data.peer_id;
    keepAlive(p, call.id, peers[p.id]);
  }
  return { chat, call, peers };
};
const record = async (c: Client, call: string, end?: typeof egress.nextEnd) => {
  ok('(recording started)', (await c.post(`/calls/${call}/recording`)).status === 200);
  if (end) egress.nextEnd = end;
  await c.del(`/calls/${call}/recording`);
};

try {
  const first = await setup([ann, bob, cat]);
  if (first.call.media !== 'livekit') throw new Error(`start the server with ${Object.entries(LIVEKIT_ENV).map(([k, v]) => `${k}=${v}`).join(' ')}`);

  // ---- several presses at once ----
  egress.startDelayMs = 400;
  const presses = await Promise.all([ann, bob, cat, ann, bob].map((c) => c.post(`/calls/${first.call.id}/recording`)));
  egress.startDelayMs = 0;
  const codes = presses.map((r) => r.status).sort();
  ok('five presses at once: one recording', codes.filter((c) => c === 200).length === 1 && codes.filter((c) => c === 409).length === 4, codes);
  await new Promise((r) => setTimeout(r, 500));
  const active = [...egress.egresses.values()].filter((e) => e.roomName === `nerp-call-${first.call.id}` && e.status === 'EGRESS_ACTIVE');
  ok('and one running in Egress (extras stopped)', active.length === 1, [...egress.egresses.values()].map((e) => e.status));
  ok('one row', (await recordings(first.call.id)).filter((r) => r.status === 'recording').length === 1);
  await ann.del(`/calls/${first.call.id}/recording`);
  await waitFor(async () => (await recordings(first.call.id)).every((r) => r.status === 'ready'), 45_000);
  ok('it is saved', (await recordings(first.call.id)).filter((r) => r.status === 'ready').length === 1);

  // ---- stopping ----
  ok('stopping when not recording', (await ann.del(`/calls/${first.call.id}/recording`)).status === 400);
  await ann.post(`/calls/${first.call.id}/recording`);
  await cat.post(`/calls/${first.call.id}/leave`, { peer_id: first.peers[cat.id] });
  ok('someone who left can\'t stop it', (await cat.del(`/calls/${first.call.id}/recording`)).status === 400);
  ok('or start one', (await cat.post(`/calls/${first.call.id}/recording`)).status === 400);
  ok('an outsider can\'t see it', (await dan.get(`/calls/${first.call.id}`)).status === 404 && (await dan.del(`/calls/${first.call.id}/recording`)).status === 404);

  // ---- Egress finishes without a file ----
  egress.nextEnd = 'complete-no-file';
  await bob.del(`/calls/${first.call.id}/recording`);
  const noFile = await waitFor(async () => (await messages(ann, first.chat)).find((m) => m.body.startsWith('The recording failed')), 45_000);
  ok('no file: the chat is told', /not in the recordings folder/.test(noFile?.body ?? ''), noFile);

  // ---- Egress's time limit ----
  await record(ann, first.call.id, 'limit-with-file');
  const limited = await waitFor(async () => (await recordings(first.call.id)).at(-1)?.status === 'ready', 45_000);
  ok('time limit reached with a file: kept', limited);
  await record(ann, first.call.id, 'limit-no-file');
  const lim2 = await waitFor(async () => (await recordings(first.call.id)).at(-1)?.status === 'failed', 45_000);
  ok('time limit reached without a file: failed', lim2 && /limit_reached/.test((await recordings(first.call.id)).at(-1)?.error ?? ''), await recordings(first.call.id));

  // ---- Egress's API down for a while ----
  const downStart = await ann.post(`/calls/${first.call.id}/recording`);
  egress.down = true;
  const downStop = await ann.del(`/calls/${first.call.id}/recording`);
  ok('stopping while Egress\'s API is down still stops it here', downStop.status === 200 && (await ann.get(`/calls/${first.call.id}`)).data.recording === null, { start: downStart, stop: downStop, rows: await recordings(first.call.id) });
  await new Promise((r) => setTimeout(r, 25_000));
  ok('while it is down the recording waits (not failed)', (await recordings(first.call.id)).at(-1)?.status === 'processing');
  egress.down = false;
  // Stop it for real now that Egress answers again (as LiveKit would on its own when the room empties).
  const pending = [...egress.egresses.values()].at(-1)!;
  if (pending.status === 'EGRESS_ACTIVE') await fetch(`${LIVEKIT_ENV.LIVEKIT_API_URL}/twirp/livekit.Egress/StopEgress`, { method: 'POST' }).catch(() => {});
  pending.status = 'EGRESS_COMPLETE';
  const { writeFileSync } = await import('node:fs');
  writeFileSync(pending.filepath, Buffer.alloc(1000, 1));
  ok('and is collected when it is back', await waitFor(async () => (await recordings(first.call.id)).at(-1)?.status === 'ready', 45_000));

  // ---- Egress loses a recording ----
  await ann.post(`/calls/${first.call.id}/recording`);
  const lost = [...egress.egresses.values()].at(-1)!;
  egress.egresses.delete(lost.egressId);
  await db.query(`UPDATE call_recordings SET started_at = now() - interval '2 hours' WHERE call_id = $1 AND status = 'recording'`, [first.call.id]);
  ok('lost by Egress: failed after a while', await waitFor(async () => (await recordings(first.call.id)).some((r) => r.status === 'failed' && r.error === 'the video server lost the recording'), 45_000));
  ok('and the call can be recorded again', (await ann.get(`/calls/${first.call.id}`)).data.recording === null && (await ann.post(`/calls/${first.call.id}/recording`)).status === 200);
  await ann.del(`/calls/${first.call.id}/recording`);

  // ---- who may download ----
  // Recordings Ann made (the recorder keeps their own file whatever happens to the chat).
  const files = (await messages(ann, first.chat)).filter((m) => m.files?.length && m.user_id === ann.id).map((m) => m.files[0].id);
  ok('recordings in the chat', files.length >= 2, files);
  ok('chat members download them', (await cat.req('GET', `/files/${files[0]}/content`, undefined, true)).status === 200);
  ok('outsiders don\'t', (await dan.req('GET', `/files/${files[0]}/content`, undefined, true)).status === 404);
  await cat.post(`/im/chats/${first.chat}/leave`);
  ok('someone who left the chat no longer does', (await cat.req('GET', `/files/${files[0]}/content`, undefined, true)).status === 404);

  // ---- a call without a chat ----
  const direct = (await ann.post('/calls', { users: [bob.id] })).data;
  const ap = (await ann.post(`/calls/${direct.id}/join`)).data.peer_id;
  const bp = (await bob.post(`/calls/${direct.id}/join`)).data.peer_id;
  keepAlive(ann, direct.id, ap);
  keepAlive(bob, direct.id, bp);
  await bob.post(`/calls/${direct.id}/recording`);
  await bob.del(`/calls/${direct.id}/recording`);
  await waitFor(async () => (await recordings(direct.id)).at(-1)?.status === 'ready', 45_000);
  const file = (await recordings(direct.id)).at(-1)?.file_id;
  ok('no chat: the recorder is told where to find it', (await bob.get('/notifications')).data.items.some((n: any) => n.kind === 'call.recording' && n.link === '/files'));
  ok('and can download it', (await bob.req('GET', `/files/${file}/content`, undefined, true)).status === 200);
  ok('the other person can\'t (it is the recorder\'s to share)', (await ann.req('GET', `/files/${file}/content`, undefined, true)).status === 404);
  await ann.post(`/calls/${direct.id}/leave`, { peer_id: ap });
  await bob.post(`/calls/${direct.id}/leave`, { peer_id: bp });
} finally {
  beats.forEach(clearInterval);
  stopServer();
  await egress.close();
  await db.end();
  await cleanupRun(stamp);
  clearRecordings();
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
