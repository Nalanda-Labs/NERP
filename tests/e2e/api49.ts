// Call controls and history: the host mutes everyone and decides whether the others may use
// their microphone, camera and screen; everyone sees their calls, with missed ones marked.
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 700)); };
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const user = async (first: string) => {
  const u = (await admin.post('/users', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123', first_name: first, last_name: `Calls${stamp}` })).data;
  const c = new Client();
  await c.post('/auth/login', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123' });
  c.id = u.id;
  return c;
};
const [ann, bob, cat] = await Promise.all(['Ann', 'Bob', 'Cat'].map(user));

try {
  const chat = (await ann.post('/im/chats', { title: `Standup ${stamp}`, members: [bob.id, cat.id] })).data.id;
  let r = await ann.post('/calls', { chat_id: chat });
  const call = r.data.id;
  ok('the caller is the host; everything allowed', r.data.host_id === ann.id && r.data.allow.audio && r.data.allow.video && r.data.allow.screen, r.data);
  const a = (await ann.post(`/calls/${call}/join`, {})).data.peer_id;
  const b = (await bob.post(`/calls/${call}/join`, {})).data.peer_id;
  const live = await bob.ws();
  ok('only the host controls the call', (await bob.post(`/calls/${call}/controls`, { mute_all: true })).status === 400);
  r = await ann.post(`/calls/${call}/controls`, { mute_all: true });
  await new Promise((res) => setTimeout(res, 700));
  const ev = live.events.find((e: any) => e.type === 'call.controls' || e.kind === 'call.controls');
  ok('mute everyone reaches those in the call', r.status === 200 && !!ev && (ev.data ?? ev.payload)?.mute_all === true, live.events.slice(-3));
  r = await ann.post(`/calls/${call}/controls`, { allow_audio: false, allow_screen: false });
  ok('the host turns microphones and screen sharing off for the others', !r.data.allow.audio && r.data.allow.video && !r.data.allow.screen, r.data);
  ok('those joining see it', !(await bob.get(`/calls/${call}`)).data.allow.audio);
  live.close();
  await ann.post(`/calls/${call}/leave`, { peer_id: a });
  await bob.post(`/calls/${call}/leave`, { peer_id: b });

  r = await bob.get('/calls');
  const mine = r.data.items.find((c: any) => c.id === call);
  ok('call history: who, the chat, how long, joined', mine && !mine.outgoing && mine.joined && !mine.missed && mine.chat_title === `Standup ${stamp}` && mine.started_by_name.startsWith('Ann') && mine.seconds >= 0, mine);
  r = await cat.get('/calls');
  ok('a call I never joined is missed', r.data.items.find((c: any) => c.id === call)?.missed === true, r.data.items[0]);
  ok('missed calls only', (await cat.get('/calls?missed=true')).data.items.some((c: any) => c.id === call) && !(await bob.get('/calls?missed=true')).data.items.some((c: any) => c.id === call));
  ok('the caller\'s is outgoing', (await ann.get('/calls')).data.items.find((c: any) => c.id === call)?.outgoing === true);
  ok('others don\'t see it', !(await admin.get('/calls')).data.items.some((c: any) => c.id === call));
} finally {
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
