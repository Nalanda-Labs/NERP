// Calls through LiveKit, and recording them: a call goes through the media server when it
// answers (and is direct when it doesn't); browsers get a token for the call's room only;
// recording: who may start and stop it, the "recording" state everyone sees, the finished file
// posted in the call's chat and the recorder told, a recording service that isn't running, a
// recording that fails, and the call ending stopping its recording.
import { Client, waitFor } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { LIVEKIT_ENV, LIVEKIT_KEY, LIVEKIT_SECRET, LIVEKIT_SERVER, RECORDINGS_DIR, clearRecordings, startFakeEgress, startLiveKit } from './livekit-server.ts';
import { createHmac } from 'node:crypto';
import { existsSync, readdirSync } from 'node:fs';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 700)); };
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const user = async (first: string) => {
  const u = (await admin.post('/users', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123', first_name: first, last_name: 'Livekit' })).data;
  const c = new Client();
  await c.post('/auth/login', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123' });
  c.id = u.id;
  return c;
};
const ann = await user('Ann');
const bob = await user('Bob');
const cat = await user('Cat');
const chatOf = async () => (await ann.post('/im/chats', { title: `Rec ${stamp}`, members: [bob.id, cat.id] })).data.id;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

if (!LIVEKIT_SERVER) { console.log('skipped: livekit-server is not installed'); await cleanupRun(stamp); process.exit(0); }

// Without the media server answering, calls are direct.
let probe = (await ann.post('/calls', { users: [bob.id] })).data;
const configured = probe.media !== undefined;
await ann.post(`/calls/${probe.id}/join`).then((r) => ann.post(`/calls/${probe.id}/leave`, { peer_id: r.data.peer_id }));
const stopServer = await startLiveKit();
const egress = await startFakeEgress();
await sleep(5500); // NERP re-checks the server at most every 5 seconds
probe = (await ann.post('/calls', { users: [bob.id] })).data;
if (!configured || probe.media !== 'livekit') {
  await ann.post(`/calls/${probe.id}/join`).then((r) => ann.post(`/calls/${probe.id}/leave`, { peer_id: r.data.peer_id }));
  stopServer(); await egress.close(); await cleanupRun(stamp);
  console.log(`skipped: start the server with ${Object.entries(LIVEKIT_ENV).map(([k, v]) => `${k}=${v}`).join(' ')}`);
  process.exit(0);
}
const jwt = (t: string) => {
  const [h, p, sig] = t.split('.');
  return { claims: JSON.parse(Buffer.from(p, 'base64url').toString()), valid: createHmac('sha256', LIVEKIT_SECRET).update(`${h}.${p}`).digest('base64url') === sig };
};

try {
  ok('with the media server up, calls go through it', probe.media === 'livekit' && probe.can_record === true && probe.recording === null, probe);
  let r = await ann.post(`/calls/${probe.id}/join`);
  const annPeer = r.data.peer_id;
  ok('joining gives the address and a token', r.data.livekit?.url === LIVEKIT_ENV.LIVEKIT_URL && typeof r.data.livekit?.token === 'string' && !r.data.ice_servers, r.data);
  const t = jwt(r.data.livekit.token);
  ok('the token is signed with the secret, for this call\'s room only', t.valid && t.claims.iss === LIVEKIT_KEY && t.claims.sub === annPeer
    && t.claims.video.room === `nerp-call-${probe.id}` && t.claims.video.roomJoin === true && !t.claims.video.roomAdmin && !t.claims.video.roomRecord, t.claims);
  ok('it says who is joining', t.claims.name === 'Ann Livekit' && JSON.parse(t.claims.metadata).user_id === ann.id);
  ok('it runs out', t.claims.exp - Date.now() / 1000 < 7 * 3600);
  await ann.post(`/calls/${probe.id}/leave`, { peer_id: annPeer });

  // ---- recording ----
  const chat = await chatOf();
  const call = (await ann.post('/calls', { chat_id: chat })).data;
  ok('a call from a chat goes through it too', call.media === 'livekit');
  ok('recording needs you in the call', (await ann.post(`/calls/${call.id}/recording`)).status === 400);
  ok('outsiders cannot', (await (await user('Dan')).post(`/calls/${call.id}/recording`)).status === 404);
  const annIn = (await ann.post(`/calls/${call.id}/join`)).data.peer_id;
  const bobIn = (await bob.post(`/calls/${call.id}/join`)).data.peer_id;
  const bobWs = await bob.ws();
  egress.unavailable = true;
  r = await ann.post(`/calls/${call.id}/recording`);
  ok('no recording service: said plainly', r.status === 400 && /recording service \(LiveKit Egress\) is not running/.test(r.data.error?.message), r.data);
  r = await ann.post(`/calls/${call.id}/recording`);
  ok('recording starts', r.status === 200 && r.data.recording?.started_by === ann.id && r.data.recording.started_by_name === 'Ann Livekit', r.data);
  const eg = [...egress.egresses.values()].at(-1)!;
  ok('Egress records the call\'s room to an MP4 in the folder', eg.roomName === `nerp-call-${call.id}` && eg.filepath.startsWith(RECORDINGS_DIR + '/') && eg.filepath.endsWith('.mp4'), eg);
  ok('every request was signed and allowed', egress.refused === 0);
  ok('everyone in the call is told', !!(await waitFor(() => bobWs.events.find((e) => e.type === 'call.recording' && e.data.recording?.started_by === ann.id), 5000)));
  ok('the call shows it', (await bob.get(`/calls/${call.id}`)).data.recording?.started_by === ann.id);
  ok('only one recording at a time', (await bob.post(`/calls/${call.id}/recording`)).status === 409);
  ok('the chat notes it', (await bob.get(`/im/chats/${chat}/messages`)).data.items.some((m: any) => m.body === 'Ann Livekit started recording the call'));
  r = await bob.del(`/calls/${call.id}/recording`);
  ok('anyone in the call can stop it', r.status === 200 && eg.status !== 'EGRESS_ACTIVE', r.data);
  ok('stopped for everyone', (await ann.get(`/calls/${call.id}`)).data.recording === null);
  const posted = await waitFor(async () => (await ann.get(`/im/chats/${chat}/messages`)).data.items.find((m: any) => m.files?.length && m.body.startsWith('Recording of the call')), 45_000);
  ok('when ready: posted in the chat by whoever recorded it', posted && posted.user_id === ann.id && /^Call recording .*\.mp4$/.test(posted.files[0].name) && posted.files[0].mime === 'video/mp4', posted);
  ok('with its length', /\(1 min \d+ s\)/.test(posted?.body ?? ''), posted?.body);
  const fileId = posted?.files[0].id;
  ok('chat members can download it', (await cat.req('GET', `/files/${fileId}/content`, undefined, true)).status === 200);
  const got = await bob.req('GET', `/files/${fileId}/content`, undefined, true);
  ok('the whole file', got.status === 200 && (await got.arrayBuffer()).byteLength === 200_000);
  ok('the recorder is told', (await ann.get('/notifications')).data.items.some((n: any) => n.kind === 'call.recording' && n.link === `/messenger?chat=${chat}`));
  ok('the folder is cleared', !existsSync(eg.filepath));

  // ---- a recording that fails ----
  await bob.post(`/calls/${call.id}/recording`);
  egress.failNext = true;
  await bob.del(`/calls/${call.id}/recording`);
  const failed = await waitFor(async () => (await ann.get(`/im/chats/${chat}/messages`)).data.items.find((m: any) => m.body.startsWith('The recording failed')), 45_000);
  ok('a failed recording is noted in the chat', failed?.body === 'The recording failed: pipeline error: encoder stopped', failed);

  // ---- the call ends while recording ----
  await ann.post(`/calls/${call.id}/recording`);
  const last = [...egress.egresses.values()].at(-1)!;
  await ann.post(`/calls/${call.id}/leave`, { peer_id: annIn });
  ok('recording goes on while someone is in', last.status === 'EGRESS_ACTIVE');
  await bob.post(`/calls/${call.id}/leave`, { peer_id: bobIn });
  ok('the call ending stops the recording', last.status !== 'EGRESS_ACTIVE');
  const second = await waitFor(async () => (await ann.get(`/im/chats/${chat}/messages`)).data.items.filter((m: any) => m.files?.length && m.body.startsWith('Recording of the call')).length === 2, 45_000);
  ok('and it is still saved', second);
  ok('no recording after the call', (await ann.post(`/calls/${call.id}/recording`)).status === 400);
  bobWs.close();

  // ---- the media server goes away ----
  stopServer();
  await sleep(5500);
  const direct = (await ann.post('/calls', { users: [bob.id] })).data;
  ok('the media server down: calls are direct again', direct.media === 'p2p' && direct.can_record === false, direct);
  r = await ann.post(`/calls/${direct.id}/join`);
  ok('with the usual setup', Array.isArray(r.data.ice_servers) && !r.data.livekit);
  ok('direct calls are not recorded', (await ann.post(`/calls/${direct.id}/recording`)).status === 400);
  await ann.post(`/calls/${direct.id}/leave`, { peer_id: r.data.peer_id });
} finally {
  stopServer();
  await egress.close();
  await cleanupRun(stamp);
  const left = existsSync(RECORDINGS_DIR) ? readdirSync(RECORDINGS_DIR) : [];
  if (left.length) { fail++; console.log('FAIL recordings left in the folder:', left.join(', ')); }
  clearRecordings();
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
