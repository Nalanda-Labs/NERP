// Workgroups: groups and projects (kinds, dates), open / closed / private groups, join
// requests (approve, decline, withdraw), and each group's feed, files, calendar and chat,
// with who may see and do what.
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 700)); };
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const user = async (first: string) => {
  const u = (await admin.post('/users', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123', first_name: first, last_name: 'Group' })).data;
  const c = new Client();
  await c.post('/auth/login', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123' });
  c.id = u.id;
  return c;
};
const ann = await user('Ann');
const bob = await user('Bob');
const cat = await user('Cat');
const dan = await user('Dan');
const vic = await user('Vic');
const upload = async (c: Client, name: string, text: string) => {
  const fd = new FormData(); fd.append('file', new Blob([text], { type: 'text/plain' }), name);
  return (await c.req('POST', '/files', fd)).data;
};
const range = `from=${new Date(Date.now() - 86400e3).toISOString()}&to=${new Date(Date.now() + 30 * 86400e3).toISOString()}`;

try {
  // ---- groups and projects ----
  ok('a project ends after it starts', (await ann.post('/projects', { name: `Bad ${stamp}`, kind: 'project', starts_on: '2026-05-10', ends_on: '2026-05-01' })).status === 400);
  ok('kinds are checked', (await ann.post('/projects', { name: `Bad ${stamp}`, kind: 'club' })).status === 400);
  let r = await ann.post('/projects', { name: `Launch ${stamp}`, kind: 'project', starts_on: '2026-05-01', ends_on: '2026-06-30', visibility: 'open' });
  const launch = r.data;
  ok('a project with dates', r.status === 200 && launch.kind === 'project' && launch.starts_on === '2026-05-01' && launch.ends_on === '2026-06-30', r.data);
  r = await ann.post('/projects', { name: `Design guild ${stamp}`, kind: 'group', visibility: 'closed', description: 'Talk about design', member_ids: [bob.id] });
  const guild = r.data;
  ok('a closed workgroup', r.status === 200 && guild.kind === 'group' && guild.visibility === 'closed' && guild.member_count === 2, r.data);
  const secret = (await ann.post('/projects', { name: `Secret ${stamp}`, kind: 'group', visibility: 'private' })).data;
  r = await ann.patch(`/projects/${launch.id}`, { ends_on: '2026-04-01' });
  ok('dates are checked on change too', r.status === 400);
  r = await ann.patch(`/projects/${launch.id}`, { ends_on: null });
  ok('an end date can be cleared', r.status === 200 && r.data.ends_on === null);

  let list = (await cat.get(`/projects?q=${stamp}`)).data;
  ok('closed and open groups are listed for everyone', list.some((p: any) => p.id === guild.id && p.my_role === null) && list.some((p: any) => p.id === launch.id));
  ok('private groups are not', !list.some((p: any) => p.id === secret.id));
  ok('by kind', (await cat.get(`/projects?q=${stamp}&kind=group`)).data.every((p: any) => p.kind === 'group'));
  r = await cat.get(`/projects/${guild.id}`);
  ok('outsiders see the card of a closed group', r.status === 200 && r.data.access.view === false && r.data.members.every((m: any) => m.role === 'owner') && r.data.stages.length === 0, r.data);
  ok('but not a private one', (await cat.get(`/projects/${secret.id}`)).status === 404 && (await cat.post(`/projects/${secret.id}/join`)).status === 404);

  // ---- content of a closed group is for members ----
  ok('feed: members only', (await cat.get(`/feed?project_id=${guild.id}`)).status === 404);
  ok('files: members only', (await cat.get(`/drive?space=group&project_id=${guild.id}`)).status === 404);
  ok('calendar: members only', (await cat.get(`/calendar?${range}&project_id=${guild.id}`)).status === 404);
  ok('chat: members only', (await cat.post(`/projects/${guild.id}/chat`)).status === 404);
  ok('posting: members only', (await cat.post('/feed', { body: 'hi', project_id: guild.id })).status === 404);

  // ---- join requests ----
  r = await cat.post(`/projects/${guild.id}/join`, { message: 'I draw a lot' });
  ok('asking to join', r.status === 200 && r.data.requested === true && r.data.access.view === false, r.data);
  await cat.post(`/projects/${guild.id}/join`, { message: 'again' });
  r = await ann.get(`/projects/${guild.id}/requests`);
  ok('the owners see the request once', r.data.items.length === 1 && r.data.items[0].user_id === cat.id && r.data.items[0].message === 'I draw a lot', r.data);
  ok('members cannot', (await bob.get(`/projects/${guild.id}/requests`)).status === 403);
  ok('the owner is told', (await ann.get('/notifications')).data.items.some((n: any) => n.kind === 'project.join_request' && n.title.includes('Cat Group')));
  ok('the count shows', (await ann.get(`/projects/${guild.id}`)).data.request_count === 1);
  r = await ann.post(`/projects/${guild.id}/requests/${cat.id}`, { approve: true });
  ok('approved: a member', r.status === 200 && r.data.members.some((m: any) => m.user_id === cat.id && m.role === 'member') && r.data.request_count === 0, r.data);
  ok('and now sees the feed', (await cat.get(`/feed?project_id=${guild.id}`)).status === 200);
  await dan.post(`/projects/${guild.id}/join`);
  ok('withdrawing a request', (await dan.del(`/projects/${guild.id}/requests/${dan.id}`)).status === 200 && (await ann.get(`/projects/${guild.id}/requests`)).data.items.length === 0);
  ok('only your own', (await dan.del(`/projects/${guild.id}/requests/${cat.id}`)).status === 403);
  await dan.post(`/projects/${guild.id}/join`);
  r = await ann.post(`/projects/${guild.id}/requests/${dan.id}`, { approve: false });
  ok('declined: not a member, and told', !r.data.members.some((m: any) => m.user_id === dan.id) && (await dan.get('/notifications')).data.items.some((n: any) => n.kind === 'project.join_declined'));
  ok('no request left to answer', (await ann.post(`/projects/${guild.id}/requests/${dan.id}`, { approve: true })).status === 404);
  r = await dan.post(`/projects/${launch.id}/join`);
  ok('an open group: joined at once', r.data.my_role === 'member', r.data);
  await ann.put(`/projects/${guild.id}/members/${vic.id}`, { role: 'viewer' });

  // ---- the group's feed ----
  r = await bob.post('/feed', { body: `Guild news ${stamp}`, project_id: guild.id, to_all: true });
  const post = r.data;
  ok('a post in the group', r.status === 200 && post.project_id === guild.id && post.project_name === `Design guild ${stamp}` && post.to_all === false, r.data);
  ok('in the group feed', (await ann.get(`/feed?project_id=${guild.id}`)).data.items.some((p: any) => p.id === post.id));
  ok('in members\' own feeds', (await cat.get(`/feed?q=${stamp}`)).data.items.some((p: any) => p.id === post.id));
  ok('not for outsiders', !(await dan.get(`/feed?q=${stamp}`)).data.items.some((p: any) => p.id === post.id) && (await dan.get(`/feed/${post.id}`)).status === 404);
  ok('viewers read but do not post', (await vic.get(`/feed/${post.id}`)).status === 200 && (await vic.post('/feed', { body: 'x', project_id: guild.id })).status === 403);
  ok('comments from members', (await cat.post(`/feed/${post.id}/comments`, { body: 'Nice' })).status === 200);
  const open = (await dan.post('/feed', { body: `Launch news ${stamp}`, project_id: launch.id })).data;
  ok('open group posts can be read by anyone', (await cat.get(`/feed/${open.id}`)).status === 200 && (await cat.get(`/feed?project_id=${launch.id}`)).data.items.some((p: any) => p.id === open.id));
  ok('but stay out of non-members\' feeds', !(await cat.get(`/feed?q=${stamp}`)).data.items.some((p: any) => p.id === open.id));
  ok('the group owner moderates', (await ann.del(`/feed/${open.id}`)).status === 200);

  // ---- the group's files ----
  r = await bob.post('/drive/folders', { name: `Specs ${stamp}`, space: 'group', project_id: guild.id });
  const folder = r.data;
  ok('a folder in the group drive', r.status === 200 && folder.space === 'group', r.data);
  const f1 = await upload(bob, `brief-${stamp}.txt`, 'the brief');
  r = await bob.post('/drive/files', { file_id: f1.id, parent_id: folder.id });
  const brief = r.data;
  ok('a file in it', r.status === 200 && brief.space === 'group', r.data);
  r = await ann.get(`/drive?space=group&project_id=${guild.id}`);
  ok('members browse it', r.data.items.some((i: any) => i.id === folder.id) && r.data.can_add === true, r.data);
  ok('outsiders cannot', (await dan.get(`/drive/nodes/${brief.id}`)).status === 404 && (await dan.req('GET', `/drive/nodes/${brief.id}/download`, undefined, true)).status === 404);
  ok('viewers read but do not add', (await vic.req('GET', `/drive/nodes/${brief.id}/download`, undefined, true)).status === 200
    && (await vic.post('/drive/folders', { name: 'x', space: 'group', project_id: guild.id })).status === 403);
  ok('owners manage everything', (await ann.patch(`/drive/nodes/${brief.id}`, { name: `brief-v2-${stamp}.txt` })).status === 200);
  const annFolder = (await ann.post('/drive/folders', { name: `Owner ${stamp}`, space: 'group', project_id: guild.id })).data;
  ok('members manage their own only', (await bob.patch(`/drive/nodes/${annFolder.id}`, { name: 'mine now' })).status === 403);
  const mine = (await bob.post('/drive/folders', { name: `Mine ${stamp}` })).data;
  ok('files stay in their drive', (await bob.patch(`/drive/nodes/${brief.id}`, { parent_id: mine.id })).status === 400);
  ok('a group file is readable through the file link', (await cat.req('GET', `/files/${f1.id}/content`, undefined, true)).status === 200);

  // ---- the group's calendar ----
  const start = new Date(Date.now() + 2 * 86400e3);
  r = await bob.post('/calendar/events', { title: `Guild meetup ${stamp}`, start: start.toISOString(), end: new Date(+start + 3600e3).toISOString(), project_id: guild.id });
  const ev = r.data;
  ok('an event on the group calendar', r.status === 200 && ev.project?.id === guild.id, r.data);
  r = await cat.get(`/calendar?${range}&project_id=${guild.id}`);
  ok('members see the group calendar', r.data.items.some((i: any) => i.id === ev.id), r.data);
  ok('outsiders do not', (await dan.get(`/calendar?${range}&project_id=${guild.id}`)).status === 404);
  ok('viewers do not add events', (await vic.post('/calendar/events', { title: 'x', start: start.toISOString(), end: new Date(+start + 3600e3).toISOString(), project_id: guild.id })).status === 403);
  ok('not on others\' own calendars', !(await cat.get(`/calendar?${range}`)).data.items.some((i: any) => i.id === ev.id));

  // ---- the group's chat ----
  r = await bob.post(`/projects/${guild.id}/chat`);
  const chatId = r.data.chat_id;
  ok('the group chat', r.status === 200 && !!chatId);
  ok('the same chat next time', (await ann.post(`/projects/${guild.id}/chat`)).data.chat_id === chatId);
  r = await ann.get(`/im/chats/${chatId}`);
  const people = (r.data.people ?? r.data.members ?? []).map((p: any) => p.id);
  ok('its members are the group\'s', r.data.title === `Design guild ${stamp}` && r.data.project_id === guild.id && [ann.id, bob.id, cat.id, vic.id].every((id) => people.includes(id)) && !people.includes(dan.id), r.data);
  await ann.put(`/projects/${guild.id}/members/${dan.id}`, { role: 'member' });
  ok('joining the group joins the chat', (await dan.get(`/im/chats/${chatId}`)).status === 200);
  await ann.del(`/projects/${guild.id}/members/${cat.id}`);
  ok('leaving the group leaves the chat', (await cat.get(`/im/chats/${chatId}`)).status === 404);
  await ann.patch(`/projects/${guild.id}`, { name: `Design circle ${stamp}` });
  ok('renaming the group renames the chat', (await bob.get(`/im/chats/${chatId}`)).data.title === `Design circle ${stamp}`);
  ok('its members are not edited in the messenger', (await bob.patch(`/im/chats/${chatId}`, { add: [cat.id] })).status === 400 && (await bob.post(`/im/chats/${chatId}/leave`)).status === 400);
  ok('messages work as usual', (await bob.post(`/im/chats/${chatId}/messages`, { body: 'hello group' })).status === 200);
  ok('in an open group, join first', (await cat.post(`/projects/${launch.id}/chat`)).status === 400);

  // ---- deleting a group takes its files ----
  r = await ann.del(`/projects/${guild.id}`);
  ok('a group without tasks can be deleted', r.status === 200, r.data);
  ok('its files are gone', (await bob.req('GET', `/files/${f1.id}/content`, undefined, true)).status === 404);
  ok('its posts, events and chat too', (await bob.get(`/feed/${post.id}`)).status === 404 && (await bob.get(`/calendar/events/${ev.id}`)).status === 404 && (await bob.get(`/im/chats/${chatId}`)).status === 404);
} finally {
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
