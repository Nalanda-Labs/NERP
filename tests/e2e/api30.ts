// Knowledge base: spaces (who may create, read, edit; departments with sub-departments),
// pages (tree, moving, no cycles), Markdown made safe, versions (history, restore, refusing
// to save over a newer version), search, deleting.
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 700)); };
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const user = async (first: string) => {
  const u = (await admin.post('/users', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123', first_name: first, last_name: 'Kb' })).data;
  const c = new Client();
  await c.post('/auth/login', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123' });
  c.id = u.id;
  return c;
};
const ann = await user('Ann');
const bob = await user('Bob');
const cat = await user('Cat');

try {
  const hr = (await admin.post('/departments', { name: `HR ${stamp}` })).data;
  const hrTeam = (await admin.post('/departments', { name: `HR team ${stamp}`, parent_id: hr.id })).data;
  await admin.post(`/departments/${hrTeam.id}/members`, { user_id: bob.id });

  // ---- spaces ----
  ok('only knowledge base managers create spaces', (await ann.post('/kb/spaces', { name: `X ${stamp}` })).status === 403);
  let r = await admin.post('/kb/spaces', { name: `Handbook ${stamp}`, description: 'How we work' });
  const open = r.data;
  ok('an open space', r.status === 200 && open.to_all && open.can_edit && open.can_manage);
  r = await admin.post('/kb/spaces', { name: `HR only ${stamp}`, to_all: false, readers: { departments: [hr.id] }, editors: { users: [ann.id] } });
  const hrSpace = r.data;
  ok('a restricted space', r.data.readers.departments[0] === hr.id && r.data.editors.users[0].id === ann.id, r.data);
  ok('sub-departments read it', (await bob.get(`/kb/spaces/${hrSpace.id}`)).status === 200);
  ok('others cannot', (await cat.get(`/kb/spaces/${hrSpace.id}`)).status === 404 && !(await cat.get('/kb/spaces')).data.items.some((s: any) => s.id === hrSpace.id));
  ok('editors read it too', (await ann.get(`/kb/spaces/${hrSpace.id}`)).data.can_edit === true);
  ok('readers cannot edit', (await bob.post(`/kb/spaces/${hrSpace.id}/pages`, { title: 'x' })).status === 403);
  ok('readers cannot change the space', (await bob.patch(`/kb/spaces/${hrSpace.id}`, { name: 'x' })).status === 403 && !('readers' in (await bob.get(`/kb/spaces/${hrSpace.id}`)).data));

  // ---- pages ----
  ok('a title is needed', (await ann.post(`/kb/spaces/${hrSpace.id}/pages`, { title: ' ' })).status === 400);
  r = await ann.post(`/kb/spaces/${hrSpace.id}/pages`, { title: 'Leave', body: `# Leave policy\n\nAsk **two weeks** ahead. ${stamp}\n\n<script>alert(1)</script>[x](javascript:alert(1))` });
  const leave = r.data;
  ok('page with safe HTML', leave.version === 1 && leave.html.includes('<h1>Leave policy</h1>') && leave.html.includes('<strong>two weeks</strong>') && !leave.html.includes('<script') && !leave.html.includes('href="javascript'), leave.html);
  const sick = (await ann.post(`/kb/spaces/${hrSpace.id}/pages`, { title: 'Sick leave', parent_id: leave.id, body: 'Call your manager.' })).data;
  const deep = (await ann.post(`/kb/spaces/${hrSpace.id}/pages`, { title: 'Doctor notes', parent_id: sick.id })).data;
  ok('sub-pages: path and children', deep.path.map((x: any) => x.title).join('/') === 'Leave/Sick leave' && (await ann.get(`/kb/pages/${leave.id}`)).data.children[0].id === sick.id);
  ok('a page cannot go under itself', (await ann.patch(`/kb/pages/${leave.id}`, { parent_id: deep.id })).status === 400);
  const other = (await admin.post(`/kb/spaces/${open.id}/pages`, { title: 'Other' })).data;
  ok('nor into another space', (await ann.patch(`/kb/pages/${sick.id}`, { parent_id: other.id })).status === 400);
  ok('readers read pages', (await bob.get(`/kb/pages/${leave.id}`)).data.can_edit === false && (await cat.get(`/kb/pages/${leave.id}`)).status === 404);

  // ---- versions ----
  r = await ann.patch(`/kb/pages/${leave.id}`, { body: `Ask three weeks ahead. ${stamp}`, version: 1 });
  ok('saved as version 2', r.data.version === 2 && r.data.updated_by_name === 'Ann Kb');
  r = await admin.patch(`/kb/pages/${leave.id}`, { body: 'old edit', version: 1 });
  ok('saving over a newer version is refused', r.status === 409 && /Ann Kb saved a newer version/.test(JSON.stringify(r.data)), r.data);
  ok('nothing changed needs no version', (await ann.patch(`/kb/pages/${leave.id}`, { body: `Ask three weeks ahead. ${stamp}`, version: 2 })).data.version === 2);
  r = await ann.get(`/kb/pages/${leave.id}/versions`);
  ok('history', r.data.items.map((v: any) => v.version).join() === '2,1' && r.data.items[0].saved_by === 'Ann Kb');
  ok('an old version', (await bob.get(`/kb/pages/${leave.id}/versions/1`)).data.html.includes('two weeks'));
  ok('readers cannot restore', (await bob.post(`/kb/pages/${leave.id}/versions/1/restore`, {})).status === 403);
  r = await ann.post(`/kb/pages/${leave.id}/versions/1/restore`, {});
  ok('restored as version 3', r.data.version === 3 && r.data.html.includes('two weeks') && (await ann.get(`/kb/pages/${leave.id}/versions`)).data.items.length === 3);

  // ---- search ----
  r = await bob.get(`/kb/search?q=${stamp}`);
  ok('search finds readable pages with a snippet', r.data.items.length === 1 && r.data.items[0].title === 'Leave' && r.data.items[0].snippet.includes('two weeks'), r.data);
  ok('not pages you cannot read', (await cat.get(`/kb/search?q=${stamp}`)).data.items.length === 0);
  ok('preview renders like a page', (await cat.post('/kb/preview', { body: '**hi**' })).data.html.includes('<strong>hi</strong>'));

  // ---- changing access, deleting ----
  await admin.patch(`/kb/spaces/${hrSpace.id}`, { to_all: true });
  ok('opened to everyone', (await cat.get(`/kb/pages/${leave.id}`)).status === 200);
  r = await ann.req('DELETE', `/kb/pages/${sick.id}`);
  ok('deleting a page takes its sub-pages', r.status === 200 && (await ann.get(`/kb/pages/${deep.id}`)).status === 404);
  ok('only owners delete spaces', (await ann.req('DELETE', `/kb/spaces/${hrSpace.id}`)).status === 403);
  ok('deleted', (await admin.req('DELETE', `/kb/spaces/${hrSpace.id}`)).status === 200 && (await ann.get(`/kb/pages/${leave.id}`)).status === 404);
} finally {
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
