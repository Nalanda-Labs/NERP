// Video calls: starting from a chat (ringing, notification, chat note, joining an ongoing
// call), who may join, passing setup messages between browsers (checked), declining,
// heartbeats and silent browsers, leaving and the call ending.
import pg from 'pg';
import { Client, waitFor } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { dbUrl } from '../db/cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 700)); };
const stamp = Date.now().toString(36);
const db = new pg.Client({ connectionString: dbUrl }); await db.connect();
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const user = async (first: string) => {
  const u = (await admin.post('/users', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123', first_name: first, last_name: 'Call' })).data;
  const c = new Client();
  await c.post('/auth/login', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123' });
  c.id = u.id;
  return c;
};
const ann = await user('Ann');
const bob = await user('Bob');
const cat = await user('Cat');
const seen = (ws: any, type: string, f: (d: any) => boolean = () => true) => waitFor(async () => ws.events.find((e: any) => e.type === type && f(e.data)), 5000);

try {
  const chat = (await ann.post(`/im/direct/${bob.id}`, {})).data;
  const annWs = await ann.ws();
  const bobWs = await bob.ws();

  // ---- starting ----
  ok('only in your own chats', (await cat.post('/calls', { chat_id: chat.id })).status === 404);
  let r = await ann.post('/calls', { chat_id: chat.id });
  const call = r.data;
  ok('call started', r.status === 200 && call.people.length === 2 && call.in_call.length === 0, r.data);
  ok('Bob\'s browser rings', !!(await seen(bobWs, 'call.ring', (d) => d.call_id === call.id && d.from === 'Ann Call')));
  ok('and he is notified', !!(await waitFor(async () => (await bob.get('/notifications')).data.items?.find((n: any) => n.title === 'Ann Call is calling'), 5000)));
  ok('the chat says so', (await bob.get(`/im/chats/${chat.id}/messages`)).data.items.some((m: any) => m.body === 'Ann started a call'));
  ok('outsiders can\'t see or join it', (await cat.get(`/calls/${call.id}`)).status === 404 && (await cat.post(`/calls/${call.id}/join`, {})).status === 404);

  // ---- joining ----
  const a = (await ann.post(`/calls/${call.id}/join`, {})).data;
  ok('first in: nobody to call', a.peer_id && a.peers.length === 0 && Array.isArray(a.ice_servers));
  ok('starting again joins the call going on', (await bob.post('/calls', { chat_id: chat.id })).data.id === call.id);
  const b = (await bob.post(`/calls/${call.id}/join`, {})).data;
  ok('second in: offers to the first', b.peers.length === 1 && b.peers[0].peer_id === a.peer_id && b.peers[0].name === 'Ann Call', b);
  ok('the first hears someone joined', !!(await seen(annWs, 'call.peer', (d) => d.event === 'joined' && d.peer_id === b.peer_id)));

  // ---- setup messages ----
  r = await bob.post(`/calls/${call.id}/signal`, { from: b.peer_id, to: a.peer_id, kind: 'offer', data: { type: 'offer', sdp: `v=0 ${stamp}` } });
  ok('passed on', r.status === 200 && !!(await seen(annWs, 'call.signal', (d) => d.from === b.peer_id && d.to === a.peer_id && d.kind === 'offer' && d.data.sdp === `v=0 ${stamp}`)));
  ok('only from your own browser', (await bob.post(`/calls/${call.id}/signal`, { from: a.peer_id, to: b.peer_id, kind: 'answer', data: {} })).status === 400);
  ok('only to someone in the call', (await bob.post(`/calls/${call.id}/signal`, { from: b.peer_id, to: '00000000-0000-0000-0000-000000000123', kind: 'ice', data: {} })).status === 400);
  ok('only setup messages', (await bob.post(`/calls/${call.id}/signal`, { from: b.peer_id, to: a.peer_id, kind: 'chat', data: {} })).status === 400);
  ok('not too large', (await bob.post(`/calls/${call.id}/signal`, { from: b.peer_id, to: a.peer_id, kind: 'ice', data: { x: 'x'.repeat(70000) } })).status === 400);
  ok('heartbeat', (await ann.post(`/calls/${call.id}/heartbeat`, { peer_id: a.peer_id })).status === 200);

  // ---- a browser that fell silent ----
  await db.query("UPDATE call_peers SET seen_at = now() - interval '2 minutes' WHERE peer_id = $1", [a.peer_id]);
  await bob.post(`/calls/${call.id}/heartbeat`, { peer_id: b.peer_id });
  ok('is dropped', (await ann.post(`/calls/${call.id}/heartbeat`, { peer_id: a.peer_id })).status === 400 && !!(await seen(bobWs, 'call.peer', (d) => d.event === 'left' && d.peer_id === a.peer_id)));
  const a2 = (await ann.post(`/calls/${call.id}/join`, {})).data;
  ok('and can come back', a2.peers.length === 1 && a2.peer_id !== a.peer_id);

  // ---- leaving ----
  await ann.post(`/calls/${call.id}/leave`, { peer_id: a2.peer_id });
  ok('the call goes on while someone is in', (await ann.get(`/calls/${call.id}`)).data.ended_at === null);
  await bob.post(`/calls/${call.id}/leave`, { peer_id: b.peer_id });
  r = await ann.get(`/calls/${call.id}`);
  ok('ended when the last leaves', r.data.ended_at && r.data.in_call.length === 0);
  ok('the chat says how long', (await ann.get(`/im/chats/${chat.id}/messages`)).data.items.at(-1).body.startsWith('Call ended · '));
  ok('everyone hears it ended', !!(await seen(annWs, 'call.ended', (d) => d.call_id === call.id)));
  ok('no joining an ended call', (await ann.post(`/calls/${call.id}/join`, {})).status === 400);

  // ---- calling people directly; declining ----
  ok('call active colleagues', (await ann.post('/calls', { users: ['00000000-0000-0000-0000-000000000123'] })).status === 400);
  const direct = (await ann.post('/calls', { users: [cat.id] })).data;
  ok('a call without a chat', direct.chat_id === null && direct.people.length === 2);
  await cat.post(`/calls/${direct.id}/decline`, {});
  ok('declining tells the caller', !!(await seen(annWs, 'call.declined', (d) => d.call_id === direct.id && d.name === 'Cat Call')));
  ok('and shows on the call', (await ann.get(`/calls/${direct.id}`)).data.people.find((p: any) => p.id === cat.id).declined);
  annWs.close();
  bobWs.close();
} finally {
  await db.end();
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
