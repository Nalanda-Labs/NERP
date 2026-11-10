// Messenger depth: roles in a chat, open chats (found, read and joined), channels (managers
// post, members comment), the general chat and department chats following the company,
// polls, reactions, voice messages and stickers, tasks and events from a chat, clearing
// history, automatic deletion, muting and folders.
import { Client, waitFor } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 700)); };
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const user = async (first: string) => {
  const u = (await admin.post('/users', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123', first_name: first, last_name: `Im${stamp}` })).data;
  const c = new Client();
  await c.post('/auth/login', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123' });
  c.id = u.id;
  return c;
};
const [ann, bob, cat, dan] = await Promise.all(['Ann', 'Bob', 'Cat', 'Dan'].map(user));
const texts = async (c: Client, chat: string, q = '') => ((await c.get(`/im/chats/${chat}/messages${q}`)).data.items ?? []).map((m: any) => m.body);
const err = (r: any) => r.data?.error?.message ?? '';

try {
  // ---- roles ----
  let r = await ann.post('/im/chats', { title: `Crew ${stamp}`, members: [bob.id, cat.id] });
  const crew = r.data.id;
  ok('the maker of a group chat is its owner', r.data.my_role === 'owner' && r.data.people.find((p: any) => p.id === ann.id)?.role === 'owner', r.data);
  ok('a member can\'t remove people or change settings', (await bob.patch(`/im/chats/${crew}`, { remove: [cat.id] })).status === 400 && (await bob.patch(`/im/chats/${crew}`, { description: 'x' })).status === 400);
  ok('only the owner gives roles', (await bob.put(`/im/chats/${crew}/members/${cat.id}`, { role: 'admin' })).status === 400);
  r = await ann.put(`/im/chats/${crew}/members/${bob.id}`, { role: 'admin' });
  ok('the owner makes an admin', r.data.people.find((p: any) => p.id === bob.id)?.role === 'admin');
  ok('an admin removes people', (await bob.patch(`/im/chats/${crew}`, { remove: [cat.id] })).status === 200 && (await cat.get(`/im/chats/${crew}`)).status === 404);
  ok('but not the owner', (await bob.patch(`/im/chats/${crew}`, { remove: [ann.id] })).data.people?.some((p: any) => p.id === ann.id));
  r = await ann.patch(`/im/chats/${crew}`, { post_policy: 'managers' });
  ok('only managers write', r.status === 200 && (await cat.get(`/im/chats/${crew}`)).status === 404);
  await ann.patch(`/im/chats/${crew}`, { add: [dan.id] });
  r = await dan.post(`/im/chats/${crew}/messages`, { body: 'hi' });
  ok('a member can\'t write when only managers may', r.status === 400 && (await bob.post(`/im/chats/${crew}/messages`, { body: `Admins speak ${stamp}` })).status === 200, r.data);
  await ann.patch(`/im/chats/${crew}`, { post_policy: 'all' });
  await ann.post(`/im/chats/${crew}/leave`);
  r = await bob.get(`/im/chats/${crew}`);
  ok('an owner who leaves hands over to an admin', r.data.my_role === 'owner', r.data);

  // ---- open chats ----
  r = await ann.post('/im/chats', { title: `Book club ${stamp}`, visibility: 'open', description: 'Novels' });
  const club = r.data.id;
  ok('an open chat may start with just its owner', r.status === 200 && r.data.visibility === 'open', r.data);
  await ann.post(`/im/chats/${club}/messages`, { body: `First book ${stamp}` });
  r = await dan.get(`/im/open?q=book club ${stamp}`);
  ok('others find it', r.data.items.some((c: any) => c.id === club && c.description === 'Novels'), r.data);
  r = await dan.get(`/im/chats/${club}`);
  ok('and read it before joining', r.status === 200 && r.data.my_role === null && r.data.can.join && (await texts(dan, club)).includes(`First book ${stamp}`), r.data);
  ok('but not write', (await dan.post(`/im/chats/${club}/messages`, { body: 'x' })).status === 400);
  r = await dan.post(`/im/chats/${club}/join`);
  ok('join it', r.data.my_role === 'member' && (await dan.post(`/im/chats/${club}/messages`, { body: `Joined ${stamp}` })).status === 200);
  ok('private chats aren\'t found or readable', !(await dan.get(`/im/open?q=Crew ${stamp}`)).data.items.length && (await cat.get(`/im/chats/${crew}/messages`)).status === 404);

  // ---- channels ----
  r = await ann.post('/im/chats', { title: `News ${stamp}`, kind: 'channel', visibility: 'open' });
  const news = r.data.id;
  ok('a channel: only managers post', r.data.kind === 'channel' && r.data.post_policy === 'managers' && r.data.can.post);
  await bob.post(`/im/chats/${news}/join`);
  r = await bob.post(`/im/chats/${news}/messages`, { body: 'me too' });
  ok('a reader can\'t post', r.status === 400 && /comment/.test(err(r)), r.data);
  const post = (await ann.post(`/im/chats/${news}/messages`, { body: `We launched ${stamp}` })).data;
  r = await bob.post(`/im/chats/${news}/messages`, { body: `Congrats ${stamp}`, parent_id: post.id });
  ok('but comments under a post', r.status === 200 && r.data.parent_id === post.id, r.data);
  r = await bob.get(`/im/chats/${news}/messages`);
  ok('comments stay under the post, with a count', r.data.items.every((m: any) => m.parent_id === null) && r.data.items.find((m: any) => m.id === post.id)?.comment_count === 1, r.data.items);
  ok('the comments of a post', (await texts(ann, news, `?parent_id=${post.id}`)).includes(`Congrats ${stamp}`));
  const comment = (await bob.get(`/im/chats/${news}/messages?parent_id=${post.id}`)).data.items[0];
  ok('no comments on comments', (await ann.post(`/im/chats/${news}/messages`, { body: 'x', parent_id: comment.id })).status === 400);
  ok('channels can\'t be opened up to everyone writing', (await ann.patch(`/im/chats/${news}`, { post_policy: 'all' })).status === 400);
  ok('a manager deletes others\' comments', (await ann.del(`/im/messages/${comment.id}`)).status === 200);

  // ---- general chat ----
  r = await bob.get('/im/general');
  const general = r.data.id;
  ok('the general chat: everyone is in it', r.status === 200 && r.data.is_general && r.data.my_role === 'member', r.data);
  ok('it is the same for everyone', (await dan.get('/im/general')).data.id === general);
  const eve = await user('Eve');
  ok('a new employee is in it straight away', (await eve.get(`/im/chats/${general}`)).status === 200);
  ok('nobody leaves it', (await bob.post(`/im/chats/${general}/leave`)).status === 400);
  ok('only administrators change it', (await bob.patch(`/im/chats/${general}`, { post_policy: 'managers' })).status === 400 && (await admin.patch(`/im/chats/${general}`, { description: 'Everyone in the company' })).status === 200);
  await admin.patch(`/users/${eve.id}`, { is_active: false });
  ok('a deactivated employee leaves it', (await admin.get(`/im/chats/${general}`)).data.people.every((p: any) => p.id !== eve.id));
  ok('making company-wide chats is for administrators', (await ann.post('/im/chats', { title: `All ${stamp}`, audience: 'everyone' })).status === 400);

  // ---- department chats ----
  const dept = (await admin.post('/departments', { name: `Ops ${stamp}`, head_user_id: ann.id })).data;
  const sub = (await admin.post('/departments', { name: `Ops night ${stamp}`, parent_id: dept.id })).data;
  await admin.post(`/departments/${dept.id}/members`, { user_id: bob.id });
  await admin.post(`/departments/${sub.id}/members`, { user_id: cat.id });
  ok('only the head makes a department chat', (await dan.post('/im/chats', { title: `Ops ${stamp}`, audience: 'department', department_id: dept.id })).status === 400);
  r = await ann.post('/im/chats', { title: `Ops chat ${stamp}`, audience: 'department', department_id: dept.id });
  const ops = r.data.id;
  const ids = (d: any) => d.people.map((p: any) => p.id).sort();
  ok('its members: the department and its sub-departments', JSON.stringify(ids(r.data)) === JSON.stringify([ann.id, bob.id, cat.id].sort()), r.data.people);
  await admin.post(`/departments/${sub.id}/members`, { user_id: dan.id });
  ok('someone joining the department joins the chat', (await dan.get(`/im/chats/${ops}`)).data.my_role === 'member');
  await admin.del(`/departments/${dept.id}/members/${bob.id}`);
  ok('someone leaving it leaves the chat', (await bob.get(`/im/chats/${ops}`)).status === 404);
  ok('its members aren\'t edited by hand', (await ann.patch(`/im/chats/${ops}`, { add: [bob.id] })).status === 400 && (await cat.post(`/im/chats/${ops}/leave`)).status === 400);
  r = await ann.post('/im/chats', { title: `Ops news ${stamp}`, kind: 'channel', audience: 'department', department_id: dept.id });
  ok('a department channel', r.data.kind === 'channel' && r.data.people.length === 3 && !(await cat.get(`/im/chats/${r.data.id}`)).data.can.post, r.data);

  // ---- polls ----
  r = await ann.post(`/im/chats/${club}/polls`, { question: `Next book ${stamp}?`, options: ['Dune', 'Emma', ' '] });
  const poll = r.data.id;
  ok('a poll (blank answers dropped)', r.status === 200 && r.data.kind === 'poll' && r.data.poll.options.length === 2, r.data);
  ok('a poll needs two answers', (await ann.post(`/im/chats/${club}/polls`, { question: 'q', options: ['a'] })).status === 400);
  ok('one answer unless multiple', (await dan.post(`/im/messages/${poll}/vote`, { options: [0, 1] })).status === 400);
  await dan.post(`/im/messages/${poll}/vote`, { options: [1] });
  r = await ann.post(`/im/messages/${poll}/vote`, { options: [1] });
  ok('votes are counted, with names, and mine', r.data.poll.options[1].votes === 2 && r.data.poll.options[1].voters.length === 2 && JSON.stringify(r.data.poll.my_votes) === '[1]' && r.data.poll.voters === 2, r.data.poll);
  r = await dan.post(`/im/messages/${poll}/vote`, { options: [0] });
  ok('a vote can be changed', r.data.poll.options[0].votes === 1 && r.data.poll.options[1].votes === 1, r.data.poll);
  ok('only who asked closes it', (await dan.post(`/im/messages/${poll}/close-poll`)).status === 400 && (await ann.post(`/im/messages/${poll}/close-poll`)).data.poll.closed);
  ok('no voting once closed', (await dan.post(`/im/messages/${poll}/vote`, { options: [1] })).status === 400);
  r = await ann.post(`/im/chats/${club}/polls`, { question: `Secret ${stamp}`, options: ['a', 'b'], anonymous: true, multiple: true });
  await dan.post(`/im/messages/${r.data.id}/vote`, { options: [0, 1] });
  r = await ann.get(`/im/messages/${r.data.id}`);
  ok('anonymous polls count without names', r.data.poll.options[0].votes === 1 && r.data.poll.options[0].voters.length === 0, r.data.poll);

  // ---- reactions, stickers, voice ----
  const msg = (await ann.post(`/im/chats/${club}/messages`, { body: `React to me ${stamp}` })).data;
  await dan.post(`/im/messages/${msg.id}/react`, { emoji: '👍' });
  r = await ann.post(`/im/messages/${msg.id}/react`, { emoji: '👍' });
  ok('reactions add up, with names', r.data.reactions[0]?.count === 2 && r.data.reactions[0].mine && r.data.reactions[0].names.length === 2, r.data.reactions);
  r = await ann.post(`/im/messages/${msg.id}/react`, { emoji: '👍' });
  ok('reacting again takes it back', r.data.reactions[0]?.count === 1 && !r.data.reactions[0].mine, r.data.reactions);
  ok('only known reactions', (await ann.post(`/im/messages/${msg.id}/react`, { emoji: '🦄' })).status === 400);
  ok('stickers', (await dan.post(`/im/chats/${club}/messages`, { kind: 'sticker', body: 'party' })).data.kind === 'sticker' && (await dan.post(`/im/chats/${club}/messages`, { kind: 'sticker', body: 'nope' })).status === 400);
  const fd = new FormData();
  fd.append('file', new Blob([new Uint8Array([0x1a, 0x45, 0xdf, 0xa3, 1, 2, 3, 4])], { type: 'audio/webm' }), `voice-${stamp}.webm`);
  const up = (await dan.req('POST', '/files', fd)).data;
  r = await dan.post(`/im/chats/${club}/messages`, { kind: 'voice', file_ids: [up.id] });
  ok('a voice message is one recording', r.status === 200 && r.data.kind === 'voice' && r.data.files.length === 1, r.data);
  ok('not a text file', (await dan.post(`/im/chats/${club}/messages`, { kind: 'video', file_ids: [up.id] })).status === 400);
  ok('voice messages aren\'t edited', (await dan.patch(`/im/messages/${r.data.id}`, { body: 'x' })).status === 400);

  // ---- tasks and events from a chat ----
  r = await dan.post(`/im/chats/${club}/task`, { message_id: msg.id, responsible_id: ann.id });
  ok('a task from a message: its first line is the title', r.status === 200 && !!r.data.task_id, r.data);
  const task = (await ann.get(`/tasks/${r.data.task_id}`)).data;
  ok('and its text, quoted, the description', task.title === `React to me ${stamp}` && task.description.includes(`Book club ${stamp}`) && task.responsible_id === ann.id, task);
  ok('the chat says so', (await texts(ann, club)).some((t: string) => t.includes(`#${task.number}`)));
  const start = new Date(Date.now() + 2 * 86400e3);
  r = await dan.post(`/im/chats/${club}/event`, { title: `Meet-up ${stamp}`, start: start.toISOString(), end: new Date(+start + 3600e3).toISOString(), timezone: 'UTC' });
  ok('an event from the chat invites its members', r.status === 200 && r.data.people?.some((p: any) => p.id === ann.id && !p.owner), r.data);
  ok('not for someone outside it', (await cat.post(`/im/chats/${club}/task`, { title: 'x' })).status === 404);

  // ---- clearing, auto-delete, muting, folders ----
  ok('a member can\'t delete history for everyone', (await dan.post(`/im/chats/${club}/clear`, { everyone: true })).status === 400);
  await dan.post(`/im/chats/${club}/clear`, {});
  ok('clear history for me', (await texts(dan, club)).length === 0 && (await texts(ann, club)).length > 3);
  await ann.post(`/im/chats/${club}/messages`, { body: `After ${stamp}` });
  ok('new messages still come', JSON.stringify(await texts(dan, club)) === JSON.stringify([`After ${stamp}`]));
  await ann.post(`/im/chats/${club}/clear`, { everyone: true });
  ok('the owner deletes it for everyone', (await texts(ann, club)).length === 1 && /deleted the chat's history/.test((await texts(ann, club))[0]));
  r = await ann.patch(`/im/chats/${club}`, { auto_delete_hours: 24 });
  ok('messages deleted after a day', r.data.auto_delete_hours === 24 && (await texts(ann, club)).some((t: string) => /after 1 days?/.test(t)));
  ok('the limit is checked', (await ann.patch(`/im/chats/${club}`, { auto_delete_hours: 0 })).status === 400);
  await bob.post(`/im/chats/${crew}/messages`, { body: `Unread ${stamp}` });
  const before = (await dan.get('/im/unread')).data.messages;
  await dan.patch(`/im/chats/${crew}/me`, { muted: true });
  ok('a muted chat leaves the unread total', (await dan.get('/im/unread')).data.messages < before && (await dan.get(`/im/chats/${crew}`)).data.muted);
  r = await dan.post('/im/folders', { name: `Hobbies ${stamp}`, chat_ids: [club, crew, crypto.randomUUID()] });
  const folder = r.data.items[0];
  ok('a folder of my chats (others ignored)', folder?.chat_ids.length === 2, r.data);
  r = await dan.get(`/im/chats?folder_id=${folder.id}`);
  ok('the chat list by folder', r.data.items.length === 2 && r.data.items.every((c: any) => c.folder_ids.includes(folder.id)), r.data.items.map((c: any) => c.title));
  ok('and by kind', (await ann.get('/im/chats?kind=channel')).data.items.every((c: any) => c.kind === 'channel'));
  ok('folders are private', (await ann.patch(`/im/folders/${folder.id}`, { name: 'x' })).status === 404 && (await dan.del(`/im/folders/${folder.id}`)).data.items.length === 0);
} finally {
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
