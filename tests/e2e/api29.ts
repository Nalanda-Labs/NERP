// News feed: posting (validation, audiences: everyone, people, departments with their
// sub-departments), files, likes, comments (notifications), polls, important posts (read
// confirmations, who has read), editing and deleting, moderators, search, paging.
import { Client, waitFor } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 700)); };
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const user = async (first: string) => {
  const u = (await admin.post('/users', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123', first_name: first, last_name: 'Feed' })).data;
  const c = new Client();
  await c.post('/auth/login', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123' });
  c.id = u.id;
  return c;
};
const ann = await user('Ann');
const bob = await user('Bob');
const cat = await user('Cat');
const dan = await user('Dan');
const notes = async (c: Client, text: string) => (await c.get('/notifications')).data.items?.filter((n: any) => n.title.includes(text)) ?? [];
const sees = async (c: Client, text: string) => (await c.get(`/feed?q=${encodeURIComponent(text)}`)).data.items.some((p: any) => p.body.includes(text));

try {
  // Sales, with a sub-department Field sales; Bob is in Field sales, Cat in neither.
  const sales = (await admin.post('/departments', { name: `Sales ${stamp}` })).data;
  const field = (await admin.post('/departments', { name: `Field sales ${stamp}`, parent_id: sales.id })).data;
  await admin.post(`/departments/${field.id}/members`, { user_id: bob.id });

  // ---- posting ----
  ok('empty post refused', (await ann.post('/feed', { body: ' ' })).status === 400);
  ok('an audience is needed when not everyone', (await ann.post('/feed', { body: 'x', to_all: false })).status === 400);
  let r = await ann.post('/feed', { body: `Welcome all ${stamp}` });
  const all = r.data;
  ok('posted to everyone', r.status === 200 && all.to_all && all.author_name === 'Ann Feed', r.data);
  ok('everyone sees it', (await sees(cat, `Welcome all ${stamp}`)) && (await sees(bob, `Welcome all ${stamp}`)));
  r = await ann.post('/feed', { body: `Sales news ${stamp}`, to_all: false, departments: [sales.id] });
  ok('a department post names its audience', r.data.to.includes(`Sales ${stamp}`), r.data);
  ok('sub-departments see department posts', await sees(bob, `Sales news ${stamp}`));
  ok('others do not', !(await sees(cat, `Sales news ${stamp}`)) && (await cat.get(`/feed/${r.data.id}`)).status === 404);
  r = await ann.post('/feed', { body: `For Dan ${stamp}`, to_all: false, users: [dan.id] });
  ok('a post to one person', (await sees(dan, `For Dan ${stamp}`)) && !(await sees(bob, `For Dan ${stamp}`)));

  // ---- files ----
  const fd = new FormData(); fd.append('file', new Blob(['menu']), `menu-${stamp}.txt`);
  const file = (await ann.req('POST', '/files', fd)).data;
  r = await ann.post('/feed', { body: `Lunch menu ${stamp}`, to_all: false, users: [dan.id], file_ids: [file.id] });
  ok('a post with a file', r.data.files[0]?.name === `menu-${stamp}.txt`);
  ok('its audience can open the file', (await dan.req('GET', `/files/${file.id}/content`, undefined, true)).status === 200);
  ok('others cannot', (await cat.req('GET', `/files/${file.id}/content`, undefined, true)).status === 404);
  ok('only your own uploads', (await bob.post('/feed', { body: 'x', file_ids: [file.id] })).status === 400);

  // ---- likes and comments ----
  r = await bob.post(`/feed/${all.id}/like`, {});
  ok('liked', r.data.likes === 1 && r.data.liked);
  ok('liking twice counts once', (await bob.post(`/feed/${all.id}/like`, {})).data.likes === 1);
  ok('unliked', (await bob.req('DELETE', `/feed/${all.id}/like`)).data.likes === 0);
  ok('comment needs text', (await bob.post(`/feed/${all.id}/comments`, { body: '' })).status === 400);
  r = await bob.post(`/feed/${all.id}/comments`, { body: 'Thanks!' });
  ok('commented', r.data.items.length === 1 && r.data.items[0].author_name === 'Bob Feed');
  ok('the author is told', !!(await waitFor(async () => (await notes(ann, 'Bob commented: Thanks!'))[0], 5000)));
  await cat.post(`/feed/${all.id}/comments`, { body: 'Welcome!' });
  ok('earlier commenters are told too', !!(await waitFor(async () => (await notes(bob, 'Cat commented: Welcome!'))[0], 5000)));
  const bobs = (await ann.get(`/feed/${all.id}/comments`)).data.items[0];
  ok('only your own comments', (await cat.req('DELETE', `/feed/comments/${bobs.id}`)).status === 403);
  ok('the comment count', (await ann.get(`/feed/${all.id}`)).data.comments === 2);

  // ---- polls ----
  ok('a poll needs two answers', (await ann.post('/feed', { body: 'q', poll: { question: 'Which?', options: ['One', ' '] } })).status === 400);
  r = await ann.post('/feed', { body: `Party day ${stamp}`, poll: { question: 'When?', options: ['Friday', 'Saturday'] } });
  const poll = r.data;
  ok('poll', poll.poll.options.length === 2 && poll.my_vote === null);
  ok('no such answer', (await bob.post(`/feed/${poll.id}/vote`, { option: 5 })).status === 400);
  await bob.post(`/feed/${poll.id}/vote`, { option: 0 });
  await cat.post(`/feed/${poll.id}/vote`, { option: 0 });
  r = await cat.post(`/feed/${poll.id}/vote`, { option: 1 });
  ok('one vote each; voting again changes it', r.data.votes['0'] === 1 && r.data.votes['1'] === 1 && r.data.my_vote === 1, r.data.votes);
  ok('no voting without a poll', (await bob.post(`/feed/${all.id}/vote`, { option: 0 })).status === 400);

  // ---- important ----
  r = await ann.post('/feed', { body: `Office closed Friday ${stamp}`, important: true, to_all: false, departments: [sales.id], users: [cat.id] });
  const imp = r.data;
  ok('the audience is told', !!(await waitFor(async () => (await notes(bob, `Office closed Friday ${stamp}`))[0], 5000)) && (await notes(dan, `Office closed Friday ${stamp}`)).length === 0);
  ok('waiting to be read', (await bob.get('/feed/important')).data.items.some((p: any) => p.id === imp.id) && !(await ann.get('/feed/important')).data.items.some((p: any) => p.id === imp.id));
  r = await bob.post(`/feed/${imp.id}/read`, {});
  ok('read', r.data.read_by_me && r.data.reads === 1 && !(await bob.get('/feed/important')).data.items.some((p: any) => p.id === imp.id));
  const readers = (await ann.get(`/feed/${imp.id}`)).data.readers;
  ok('the author sees who has read it', readers?.length === 2 && readers.find((x: any) => x.id === bob.id)?.read_at && !readers.find((x: any) => x.id === cat.id)?.read_at, readers);
  ok('others do not see the list', !('readers' in (await bob.get(`/feed/${imp.id}`)).data));
  ok('only important posts are confirmed', (await bob.post(`/feed/${all.id}/read`, {})).status === 400);

  // ---- editing, deleting ----
  ok('only the author edits', (await bob.patch(`/feed/${all.id}`, { body: 'mine' })).status === 403);
  r = await ann.patch(`/feed/${all.id}`, { body: `Welcome everyone ${stamp}` });
  ok('edited', r.data.body === `Welcome everyone ${stamp}` && r.data.edited_at);
  ok('others cannot delete', (await bob.req('DELETE', `/feed/${all.id}`)).status === 403);
  ok('moderators can', (await admin.req('DELETE', `/feed/${poll.id}`)).status === 200 && (await ann.get(`/feed/${poll.id}`)).status === 404);

  // ---- paging ----
  for (let i = 0; i < 3; i++) await cat.post('/feed', { body: `Page ${i} ${stamp}` });
  r = await ann.get(`/feed?limit=2&q=${stamp}`);
  ok('newest first, in pages', r.data.more && r.data.items[0].body === `Page 2 ${stamp}`, r.data.items.map((p: any) => p.body));
  r = await ann.get(`/feed?limit=2&q=${stamp}&before=${encodeURIComponent(r.data.items[1].created_at)}`);
  ok('older page', r.data.items[0].body === `Page 0 ${stamp}`, r.data.items.map((p: any) => p.body));
} finally {
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
