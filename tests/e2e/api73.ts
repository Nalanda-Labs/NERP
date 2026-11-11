// Smaller gaps: drive links to folders (browsing and downloading inside, nothing outside) and
// passwords on drive links; a colleague's files shared with me; hidden departments; the task
// list's status summary; workgroups by recent activity, with overdue tasks or unread messages.
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const user = async (first: string) => {
  const email = `${first.toLowerCase()}.${stamp}@acme.test`;
  const u = (await admin.post('/users', { email, password: 'password123', first_name: first, last_name: `Qw${stamp}` })).data;
  const c = new Client();
  await c.post('/auth/login', { email, password: 'password123' });
  c.id = u.id;
  return c;
};
const [ann, bob, cat] = await Promise.all(['Ann', 'Bob', 'Cat'].map(user));
const upload = async (c: Client, name: string, text: string) => { const fd = new FormData(); fd.append('file', new Blob([text], { type: 'text/plain' }), name); return (await c.req('POST', '/files', fd)).data.id; };
const pub = (path: string, password?: string) => fetch(`http://localhost:8080/api/public/drive/${path}`, { headers: password ? { 'X-Link-Password': password } : {} });

try {
  // ---- drive: folder links ----
  const folder = (await ann.post('/drive/folders', { name: `Shared ${stamp}`, space: 'personal' })).data;
  const sub = (await ann.post('/drive/folders', { name: 'Inner', parent_id: folder.id })).data;
  const top = (await ann.post('/drive/files', { file_id: await upload(ann, 'top.txt', 'top file'), name: 'top.txt', parent_id: folder.id })).data;
  const deep = (await ann.post('/drive/files', { file_id: await upload(ann, 'deep.txt', 'deep file'), name: 'deep.txt', parent_id: sub.id })).data;
  const outside = (await ann.post('/drive/files', { file_id: await upload(ann, 'secret.txt', 'secret'), name: `secret-${stamp}.txt`, space: 'personal' })).data;
  let r = await ann.post(`/drive/nodes/${folder.id}/links`, {});
  const token = r.data.url?.split('/d/')[1];
  ok('a link to a folder', r.status === 200 && !!token, r.data);
  let res = await pub(token);
  let info = await res.json();
  ok('anyone sees what is in it', info.kind === 'folder' && info.items.some((i: any) => i.name === 'Inner' && i.kind === 'folder') && info.items.some((i: any) => i.name === 'top.txt'), info);
  info = await (await pub(`${token}?item=${sub.id}`)).json();
  ok('and opens folders inside', info.items.some((i: any) => i.name === 'deep.txt') && info.path.map((p: any) => p.name).join('/') === `Shared ${stamp}/Inner`, info);
  res = await pub(`${token}/download?item=${deep.id}`);
  ok('and downloads files at any depth', res.status === 200 && (await res.text()) === 'deep file');
  ok('but nothing outside the folder', (await pub(`${token}/download?item=${outside.id}`)).status === 404 && (await pub(`${token}?item=${outside.id}`)).status === 404);
  ok('a folder itself is not a download', (await pub(`${token}/download`)).status === 400);
  void top;

  // ---- drive: passwords ----
  ok('passwords have a minimum', (await ann.post(`/drive/nodes/${outside.id}/links`, { password: 'abc' })).status === 400);
  r = await ann.post(`/drive/nodes/${outside.id}/links`, { password: 'open sesame' });
  const locked = r.data.url.split('/d/')[1];
  ok('the link list says it has a password', r.data.links.some((l: any) => l.password === true), r.data.links);
  ok('without it: asked for it', (await pub(locked)).status === 401 && (await pub(`${locked}/download`)).status === 401);
  ok('a wrong one is refused', (await pub(locked, 'nope')).status === 401);
  res = await pub(`${locked}/download`, 'open sesame');
  ok('the right one opens it', res.status === 200 && (await res.text()) === 'secret');

  // ---- a colleague's files shared with me ----
  await ann.put(`/drive/nodes/${folder.id}/shares`, [{ user_id: bob.id, level: 'view' }]);
  r = await bob.get(`/drive?space=shared&owner=${ann.id}`);
  ok('Ann’s files shared with Bob, on her profile', r.data.items.some((i: any) => i.id === folder.id), r.data.items);
  r = await bob.get(`/drive?space=shared&owner=${cat.id}`);
  ok('not someone else’s', !r.data.items.some((i: any) => i.id === folder.id));

  // ---- hidden departments ----
  const secret = (await admin.post('/departments', { name: `Skunkworks ${stamp}`, hidden: true })).data;
  const inner = (await admin.post('/departments', { name: `Skunk lab ${stamp}`, parent_id: secret.id })).data;
  await admin.post(`/departments/${inner.id}/members`, { user_id: cat.id });
  const names = async (c: Client) => (await c.get('/departments')).data.map((d: any) => d.name);
  let seen = await names(bob);
  ok('others don’t see a hidden department or what is under it', !seen.includes(`Skunkworks ${stamp}`) && !seen.includes(`Skunk lab ${stamp}`), seen.filter((n: string) => n.includes(stamp)));
  seen = await names(cat);
  ok('its people do (from below too)', seen.includes(`Skunkworks ${stamp}`) && seen.includes(`Skunk lab ${stamp}`));
  seen = await names(admin);
  ok('and administrators', seen.includes(`Skunkworks ${stamp}`));
  r = await admin.patch(`/departments/${secret.id}`, { hidden: false });
  ok('shown again', r.data.hidden === false && (await names(bob)).includes(`Skunkworks ${stamp}`));

  // ---- task status summary ----
  for (const [t, st] of [['a', 'new'], ['b', 'in_progress'], ['c', 'in_progress'], ['d', 'done']]) {
    const x = (await ann.post('/tasks', { title: `Sum ${t} ${stamp}`, responsible_id: ann.id, deadline: t === 'a' ? new Date(Date.now() - 86400e3).toISOString() : null })).data;
    if (st !== 'new') await ann.post(`/tasks/${x.id}/status`, { status: st });
  }
  r = await ann.get(`/tasks?q=${stamp}&summary=true&limit=1`);
  ok('the list’s summary covers all of it', r.data.items.length === 1 && r.data.summary.by_status.new === 1 && r.data.summary.by_status.in_progress === 2 && r.data.summary.by_status.done === 1 && r.data.summary.overdue === 1, r.data.summary);

  // ---- workgroups by activity ----
  const g1 = (await ann.post('/projects', { name: `Quiet ${stamp}` })).data;
  const g2 = (await ann.post('/projects', { name: `Busy ${stamp}` })).data;
  await ann.post('/tasks', { title: `Late ${stamp}`, responsible_id: ann.id, project_id: g1.id, deadline: new Date(Date.now() - 86400e3).toISOString() });
  await new Promise((s) => setTimeout(s, 1100));
  await ann.post('/tasks', { title: `Fresh ${stamp}`, responsible_id: ann.id, project_id: g2.id });
  r = await ann.get(`/projects?q=${stamp}&sort=activity`);
  ok('sorted by recent activity', r.data[0]?.id === g2.id && r.data[1]?.id === g1.id, r.data.map((p: any) => p.name));
  r = await ann.get(`/projects?q=${stamp}&overdue=true`);
  ok('only those with overdue tasks', r.data.length === 1 && r.data[0].id === g1.id, r.data.map((p: any) => p.name));
  await ann.put(`/projects/${g2.id}/members/${bob.id}`, { role: 'member' });
  const chat = (await bob.post(`/projects/${g2.id}/chat`)).data.chat_id;
  await bob.post(`/im/chats/${chat}/messages`, { body: `Hi ${stamp}` });
  r = await ann.get(`/projects?q=${stamp}&unread=true`);
  ok('only those with unread messages', r.data.length === 1 && r.data[0].id === g2.id && r.data[0].unread >= 1, r.data);
} finally {
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
