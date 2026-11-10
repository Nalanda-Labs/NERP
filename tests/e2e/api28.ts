// Messenger: direct chats (one per pair), group chats (members, renaming, removing,
// leaving), messages (files, editing, deleting, paging), unread counts and read positions,
// live events over the WebSocket, and who may see what.
import { Client, waitFor } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 700)); };
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const user = async (first: string) => {
  const u = (await admin.post('/users', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123', first_name: first, last_name: 'Im' })).data;
  const c = new Client();
  await c.post('/auth/login', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123' });
  c.id = u.id;
  return c;
};
const ann = await user('Ann');
const bob = await user('Bob');
const cat = await user('Cat');
const dan = await user('Dan');

try {
  // ---- direct chats ----
  ok('not with yourself', (await ann.post(`/im/direct/${ann.id}`, {})).status === 400);
  let r = await ann.post(`/im/direct/${bob.id}`, {});
  const dm = r.data;
  ok('direct chat', r.status === 200 && dm.kind === 'direct' && dm.title === 'Bob Im' && dm.people.length === 2, r.data);
  ok('one per pair, from either side', (await bob.post(`/im/direct/${ann.id}`, {})).data.id === dm.id);
  ok('outsiders cannot read it', (await cat.get(`/im/chats/${dm.id}`)).status === 404 && (await cat.get(`/im/chats/${dm.id}/messages`)).status === 404 && (await cat.post(`/im/chats/${dm.id}/messages`, { body: 'hi' })).status === 404);

  // Live events for Bob.
  const ws = await bob.ws();
  ok('empty message refused', (await ann.post(`/im/chats/${dm.id}/messages`, { body: '  ' })).status === 400);
  r = await ann.post(`/im/chats/${dm.id}/messages`, { body: 'Lunch?' });
  const m1 = r.data;
  ok('sent', r.status === 200 && m1.body === 'Lunch?' && m1.user_name === 'Ann Im');
  const live = await waitFor(async () => ws.events.find((e: any) => e.type === 'im.message' && e.data.message.id === m1.id), 5000);
  ok('arrives live', !!live, ws.events.map((e: any) => e.type));
  ok('unread for Bob', (await bob.get('/im/unread')).data.messages === 1 && (await bob.get('/im/chats')).data.items.find((c: any) => c.id === dm.id).unread === 1);
  ok('not unread for Ann', (await ann.get('/im/unread')).data.messages === 0);
  await bob.post(`/im/chats/${dm.id}/read`, {});
  ok('read', (await bob.get('/im/unread')).data.messages === 0);
  ok('Ann sees Bob read it', new Date((await ann.get(`/im/chats/${dm.id}`)).data.people.find((p: any) => p.id === bob.id).last_read_at) >= new Date(m1.created_at));

  // ---- files ----
  const fd = new FormData(); fd.append('file', new Blob(['agenda']), `agenda-${stamp}.txt`);
  const file = (await ann.req('POST', '/files', fd)).data;
  const fd2 = new FormData(); fd2.append('file', new Blob(['x']), `cats-${stamp}.txt`);
  const catsFile = (await cat.req('POST', '/files', fd2)).data;
  ok('only your own uploads', (await ann.post(`/im/chats/${dm.id}/messages`, { body: 'x', file_ids: [catsFile.id] })).status === 400);
  ok('outsiders cannot open the file before', (await bob.req('GET', `/files/${file.id}/content`, undefined, true)).status === 404);
  r = await ann.post(`/im/chats/${dm.id}/messages`, { file_ids: [file.id] });
  ok('a file alone', r.data.files[0]?.name === `agenda-${stamp}.txt` && r.data.body === '');
  ok('the other member can open it', (await bob.req('GET', `/files/${file.id}/content`, undefined, true)).status === 200);
  ok('others still cannot', (await cat.req('GET', `/files/${file.id}/content`, undefined, true)).status === 404);
  ok('a file shows as "File" in the list', (await bob.get('/im/chats')).data.items.find((c: any) => c.id === dm.id).last_body === 'File');

  // ---- editing, deleting ----
  ok('only your own messages', (await bob.patch(`/im/messages/${m1.id}`, { body: 'hacked' })).status === 400);
  r = await ann.patch(`/im/messages/${m1.id}`, { body: 'Lunch at 1?' });
  ok('edited', r.data.body === 'Lunch at 1?' && r.data.edited_at);
  r = await ann.req('DELETE', `/im/messages/${m1.id}`);
  ok('deleted for everyone', r.data.deleted && r.data.body === '' && (await bob.get(`/im/chats/${dm.id}/messages`)).data.items.find((m: any) => m.id === m1.id).deleted);
  ok('a deleted message cannot be edited', (await ann.patch(`/im/messages/${m1.id}`, { body: 'back' })).status === 400);

  // ---- groups ----
  ok('a group needs people', (await ann.post('/im/chats', { title: `Team ${stamp}`, members: [] })).status === 400);
  r = await ann.post('/im/chats', { title: `Team ${stamp}`, members: [bob.id, cat.id] });
  const g = r.data;
  ok('group', g.kind === 'group' && g.title === `Team ${stamp}` && g.people.length === 3, r.data);
  let msgs = (await cat.get(`/im/chats/${g.id}/messages`)).data.items;
  ok('a note says who made it', msgs[0].user_id === null && msgs[0].body === `Ann made the chat “Team ${stamp}”`);
  await bob.post(`/im/chats/${g.id}/messages`, { body: 'Hi all' });
  r = await cat.patch(`/im/chats/${g.id}`, { add: [dan.id] });
  ok('any member adds people', r.data.people.length === 4 && (await dan.get(`/im/chats/${g.id}`)).status === 200);
  ok('only its maker removes people', (await cat.patch(`/im/chats/${g.id}`, { remove: [bob.id] })).status === 400);
  r = await ann.patch(`/im/chats/${g.id}`, { remove: [dan.id], title: `Core team ${stamp}` });
  ok('removed and renamed', r.data.people.length === 3 && r.data.title === `Core team ${stamp}` && (await dan.get(`/im/chats/${g.id}`)).status === 404);
  ok('direct chats cannot change', (await ann.patch(`/im/chats/${dm.id}`, { title: 'x' })).status === 400 && (await ann.post(`/im/chats/${dm.id}/leave`, {})).status === 400);
  await cat.post(`/im/chats/${g.id}/leave`, {});
  msgs = (await ann.get(`/im/chats/${g.id}/messages`)).data.items;
  ok('leaving', (await cat.get(`/im/chats/${g.id}`)).status === 404 && msgs.at(-1).body === 'Cat left', msgs.map((m: any) => m.body));
  ok('group list shows the last author', (await ann.get('/im/chats')).data.items.find((c: any) => c.id === g.id).last_body === 'Cat left');

  // ---- paging ----
  for (let i = 0; i < 6; i++) await bob.post(`/im/chats/${g.id}/messages`, { body: `n${i}` });
  r = await ann.get(`/im/chats/${g.id}/messages?limit=4`);
  ok('newest page', r.data.more && r.data.items.length === 4 && r.data.items.at(-1).body === 'n5' && r.data.items[0].body === 'n2', r.data.items.map((m: any) => m.body));
  r = await ann.get(`/im/chats/${g.id}/messages?limit=4&before=${encodeURIComponent(r.data.items[0].created_at)}`);
  ok('earlier page', r.data.items.at(-1).body === 'n1', r.data.items.map((m: any) => m.body));
  ok('unread across chats', (await ann.get('/im/unread')).data.messages >= 7);
  ok('search chats by name', (await ann.get(`/im/chats?q=core team ${stamp}`)).data.items.length === 1);
  ws.close();
} finally {
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
