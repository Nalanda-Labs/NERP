// Workgroup isolation: someone outside a closed group must not see or change anything of it by
// any route (events and posts by id, comments, likes, votes, search, file links, tasks,
// moving things out of its drive), and people who leave or are removed lose access at once.
// Members' powers stop at their role (viewers, members, owners).
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 700)); };
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const user = async (first: string) => {
  const u = (await admin.post('/users', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123', first_name: first, last_name: 'Isolated' })).data;
  const c = new Client();
  await c.post('/auth/login', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123' });
  c.id = u.id;
  return c;
};
const own = await user('Owen');   // owner
const mem = await user('Mia');    // member
const vic = await user('Vic');    // viewer
const out = await user('Otto');   // outsider
const upload = async (c: Client, name: string, text: string) => {
  const fd = new FormData(); fd.append('file', new Blob([text], { type: 'text/plain' }), name);
  return (await c.req('POST', '/files', fd)).data;
};
const status = async (p: Promise<{ status: number }>) => (await p).status;
const range = `from=${new Date(Date.now() - 86400e3).toISOString()}&to=${new Date(Date.now() + 30 * 86400e3).toISOString()}`;

try {
  for (const visibility of ['closed', 'private']) {
    const g = (await own.post('/projects', { name: `Vault ${visibility} ${stamp}`, kind: 'group', visibility, member_ids: [mem.id] })).data;
    await own.put(`/projects/${g.id}/members/${vic.id}`, { role: 'viewer' });
    const tag = `${visibility}-${stamp}`;
    const post = (await mem.post('/feed', { body: `Secret plan ${tag}`, project_id: g.id, poll: { question: 'Go?', options: ['yes', 'no'] } })).data;
    const comment = (await mem.post(`/feed/${post.id}/comments`, { body: 'inside' })).data.items.at(-1);
    const f = await upload(mem, `secret-${tag}.txt`, 'classified');
    const node = (await mem.post('/drive/files', { file_id: f.id, space: 'group', project_id: g.id })).data;
    const folder = (await mem.post('/drive/folders', { name: `Folder ${tag}`, space: 'group', project_id: g.id })).data;
    const start = new Date(Date.now() + 86400e3);
    const ev = (await mem.post('/calendar/events', { title: `Secret meeting ${tag}`, start: start.toISOString(), end: new Date(+start + 3600e3).toISOString(), project_id: g.id })).data;
    const task = (await mem.post('/tasks', { title: `Secret task ${tag}`, project_id: g.id, responsible_id: mem.id })).data;
    const chat = (await mem.post(`/projects/${g.id}/chat`)).data.chat_id;
    await mem.post(`/im/chats/${chat}/messages`, { body: `chat ${tag}` });
    ok(`${visibility}: setup`, post.id && node.id && ev.id && task.id && chat, { post, node, ev, task });

    // ---- an outsider, by every route ----
    ok(`${visibility}: post by id`, await status(out.get(`/feed/${post.id}`)) === 404);
    ok(`${visibility}: its comments`, await status(out.get(`/feed/${post.id}/comments`)) === 404);
    ok(`${visibility}: commenting`, await status(out.post(`/feed/${post.id}/comments`, { body: 'x' })) === 404);
    ok(`${visibility}: liking`, await status(out.post(`/feed/${post.id}/like`)) === 404);
    ok(`${visibility}: voting`, await status(out.post(`/feed/${post.id}/vote`, { option: 0 })) === 404);
    ok(`${visibility}: deleting a comment`, comment?.id && (await out.del(`/feed/comments/${comment.id}`)).status === 404);
    ok(`${visibility}: feed search`, !(await out.get(`/feed?q=${tag}`)).data.items?.some((p: any) => p.id === post.id));
    ok(`${visibility}: event by id`, await status(out.get(`/calendar/events/${ev.id}`)) === 404);
    ok(`${visibility}: editing the event`, (await out.patch(`/calendar/events/${ev.id}`, { title: 'pwned' })).status === 404 && (await out.del(`/calendar/events/${ev.id}`)).status === 404);
    ok(`${visibility}: drive item by id`, await status(out.get(`/drive/nodes/${node.id}`)) === 404);
    ok(`${visibility}: drive folder contents`, await status(out.get(`/drive?folder=${folder.id}`)) === 404);
    ok(`${visibility}: downloading`, (await out.req('GET', `/drive/nodes/${node.id}/download`, undefined, true)).status === 404);
    ok(`${visibility}: the file itself`, (await out.req('GET', `/files/${f.id}/content`, undefined, true)).status === 404);
    ok(`${visibility}: drive search`, !(await out.get(`/drive/search?q=${tag}`)).data.items?.some((i: any) => i.id === node.id));
    ok(`${visibility}: adding to its folder`, (await out.post('/drive/folders', { name: 'x', parent_id: folder.id })).status === 404);
    ok(`${visibility}: making a public link`, (await out.post(`/drive/nodes/${node.id}/links`, {})).status === 404);
    ok(`${visibility}: task by id`, await status(out.get(`/tasks/${task.id}`)) === 404);
    ok(`${visibility}: task list`, !((await out.get(`/tasks?project_id=${g.id}`)).data.items ?? []).some((t: any) => t.id === task.id));
    ok(`${visibility}: adding a task`, (await out.post('/tasks', { title: 'x', project_id: g.id, responsible_id: out.id })).status >= 400);
    ok(`${visibility}: the chat`, await status(out.get(`/im/chats/${chat}/messages`)) === 404);
    ok(`${visibility}: changing the group`, (await out.patch(`/projects/${g.id}`, { visibility: 'open' })).status >= 403);
    ok(`${visibility}: adding themself`, (await out.put(`/projects/${g.id}/members/${out.id}`, { role: 'owner' })).status >= 403);
    ok(`${visibility}: answering requests`, (await out.post(`/projects/${g.id}/requests/${out.id}`, { approve: true })).status >= 403);
    ok(`${visibility}: posting with an event's id trick`, (await out.post('/calendar/events', { title: 'x', start: start.toISOString(), end: new Date(+start + 3600e3).toISOString(), project_id: g.id })).status >= 403);

    // ---- roles ----
    ok(`${visibility}: viewers don't add tasks`, (await vic.post('/tasks', { title: 'x', project_id: g.id, responsible_id: vic.id })).status >= 400);
    ok(`${visibility}: viewers don't change files`, (await vic.patch(`/drive/nodes/${node.id}`, { name: 'renamed.txt' })).status === 403 && (await vic.del(`/drive/nodes/${node.id}`)).status === 403);
    ok(`${visibility}: members don't change the group`, (await mem.patch(`/projects/${g.id}`, { visibility: 'open' })).status === 403 && (await mem.put(`/projects/${g.id}/members/${mem.id}`, { role: 'owner' })).status === 403);
    ok(`${visibility}: members don't delete others' posts`, (await vic.del(`/feed/${post.id}`)).status === 403);
    ok(`${visibility}: files don't leave the group drive`, (await own.patch(`/drive/nodes/${node.id}`, { parent_id: (await own.post('/drive/folders', { name: `Mine ${tag}`, space: 'company' })).data.id })).status === 400);

    // ---- leaving and being removed ----
    await own.del(`/projects/${g.id}/members/${mem.id}`);
    ok(`${visibility}: removed: no posts`, await status(mem.get(`/feed/${post.id}`)) === 404);
    ok(`${visibility}: removed: no files (even their own upload in the group)`, (await mem.req('GET', `/drive/nodes/${node.id}/download`, undefined, true)).status === 404);
    ok(`${visibility}: removed: no events`, await status(mem.get(`/calendar/events/${ev.id}`)) === 404 || (await mem.get(`/calendar/events/${ev.id}`)).data.owner_id === mem.id);
    ok(`${visibility}: removed: no chat`, await status(mem.get(`/im/chats/${chat}/messages`)) === 404);
    ok(`${visibility}: removed: no group calendar`, await status(mem.get(`/calendar?${range}&project_id=${g.id}`)) === 404);
    await vic.post(`/projects/${g.id}/leave`);
    ok(`${visibility}: left: no posts`, await status(vic.get(`/feed/${post.id}`)) === 404);
    ok(`${visibility}: left: no files`, (await vic.req('GET', `/drive/nodes/${node.id}/download`, undefined, true)).status === 404);
    ok(`${visibility}: left: no chat`, await status(vic.get(`/im/chats/${chat}/messages`)) === 404);

    // ---- the last owner ----
    ok(`${visibility}: the last owner can't leave`, (await own.post(`/projects/${g.id}/leave`)).status === 400);
    ok(`${visibility}: or demote themself`, (await own.put(`/projects/${g.id}/members/${own.id}`, { role: 'member' })).status === 400);
    await own.del(`/tasks/${task.id}`);
  }

  // ---- requests ----
  const g = (await own.post('/projects', { name: `Requests ${stamp}`, kind: 'group', visibility: 'closed' })).data;
  const both = await Promise.all([out.post(`/projects/${g.id}/join`, { message: 'a' }), out.post(`/projects/${g.id}/join`, { message: 'b' })]);
  ok('asking twice at once: one request', both.every((r) => r.status === 200) && (await own.get(`/projects/${g.id}/requests`)).data.items.length === 1);
  ok('a long note is cut', (await (await user('Lena')).post(`/projects/${g.id}/join`, { message: 'x'.repeat(5000) })).status === 200
    && (await own.get(`/projects/${g.id}/requests`)).data.items.every((r: any) => (r.message ?? '').length <= 1000));
  await admin.patch(`/users/${out.id}`, { is_active: false });
  await own.post(`/projects/${g.id}/requests/${out.id}`, { approve: true });
  ok('a deactivated person is not let in', !(await own.get(`/projects/${g.id}`)).data.members.some((m: any) => m.user_id === out.id));
} finally {
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
