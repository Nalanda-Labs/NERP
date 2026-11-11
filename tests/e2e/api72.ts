// Guests from outside the company: invited into a chat and a workgroup by email, they set a
// password and then see only those (their messages, tasks, posts, boards shared with them
// by name, calls from their chats) — never the directory, the company feed, open chats and
// groups, things shared with everyone, or any other module.
import pg from 'pg';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { dbUrl } from '../db/cleanup.ts';

const MP = 'http://localhost:8026/api/v1';
let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const stamp = Date.now().toString(36);
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const user = async (first: string) => {
  const email = `${first.toLowerCase()}.${stamp}@acme.test`;
  const u = (await admin.post('/users', { email, password: 'password123', first_name: first, last_name: `Gst${stamp}` })).data;
  const c = new Client();
  await c.post('/auth/login', { email, password: 'password123' });
  c.id = u.id;
  return c;
};
const [ann, bob, cy] = await Promise.all(['Ann', 'Bob', 'Cy'].map(user));
const guestEmail = `gil.${stamp}@acme.test`;
const mailTo = async (addr: string) => {
  for (let i = 0; i < 40; i++) {
    const r = await (await fetch(`${MP}/search?query=${encodeURIComponent('to:' + addr)}&limit=5`)).json();
    const m = r.messages?.find((x: any) => x.Subject.includes('invited you'));
    if (m) return (await (await fetch(`${MP}/message/${m.ID}`)).json());
    await sleep(250);
  }
  return null;
};

try {
  // ---- invited into a chat ----
  const chat = (await ann.post('/im/chats', { title: `Partners ${stamp}`, members: [bob.id] })).data;
  ok('only the chat’s owners and admins invite guests', (await bob.post(`/im/chats/${chat.id}/guests`, { email: guestEmail, first_name: 'Gil', last_name: 'Out' })).status === 403);
  let r = await ann.post(`/im/chats/${chat.id}/guests`, { email: guestEmail, first_name: 'Gil', last_name: `Out${stamp}` });
  ok('a guest invited into a chat', r.status === 200 && r.data.invited === true, r.data);
  const gilId = r.data.user_id;
  r = await ann.post(`/im/chats/${chat.id}/guests`, { email: cy.email ?? `cy.${stamp}@acme.test` });
  ok('inviting an employee just adds them', r.status === 200 && r.data.invited === false && r.data.user_id === cy.id, r.data);
  const mail = await mailTo(guestEmail);
  const token = mail?.Text.match(/accept-invite\?token=([\w-]+)/)?.[1];
  ok('the guest gets the invitation', !!token);
  const gil = new Client();
  r = await gil.post('/auth/accept-invite', { token, password: 'guestpass123' });
  ok('and sets a password', r.status === 200 && r.data.user?.is_guest === true, r.data.user);
  ok('a guest has no permissions', !(r.data.user.permissions ?? []).some((p: string) => p !== '@guest'), r.data.user.permissions);

  // ---- what a guest reaches ----
  r = await gil.get('/im/chats');
  ok('their chat', r.data.items?.some((c: any) => c.id === chat.id), r.data);
  ok('they write in it', (await gil.post(`/im/chats/${chat.id}/messages`, { body: `Hello from outside ${stamp}` })).status === 200);
  for (const [path, what] of [['/users', 'the directory'], [`/users/${bob.id}`, 'profiles'], ['/crm/deal/items', 'the CRM'], ['/hr/my-day', 'HR'], ['/bi/dashboards', 'BI'], ['/kb/spaces', 'the knowledge base'], ['/drive/search?q=x', 'the drive'], ['/calendar', 'calendars'], ['/departments', 'the structure']]) {
    ok(`not ${what}`, (await gil.get(path)).status === 403);
  }
  ok('no general chat', (await gil.get('/im/general')).status === 404);
  const open = (await bob.post('/im/chats', { title: `Open talk ${stamp}`, visibility: 'open', members: [cy.id] })).data;
  ok('(an open chat to test with)', !!open.id, open);
  ok('no open chats', (await gil.get(`/im/chats/${open.id}`)).status === 404 && (await gil.get('/im/open')).data.items.length === 0 && (await gil.post(`/im/chats/${open.id}/join`)).status === 404);
  ok('no new group chats', (await gil.post('/im/chats', { title: 'x', members: [bob.id] })).status === 403);
  ok('private messages to people they share a chat with', (await gil.post(`/im/direct/${bob.id}`)).status === 200);
  const dee = await user('Dee');
  ok('but not to anyone else', (await gil.post(`/im/direct/${dee.id}`)).status === 403);
  ok('calls from their chat', [200].includes((await gil.post('/calls', { chat_id: chat.id })).status));
  ok('not calls to anyone', (await gil.post('/calls', { users: [dee.id] })).status === 400);

  // ---- a workgroup ----
  const group = (await ann.post('/projects', { name: `Joint ${stamp}`, visibility: 'open' })).data;
  const other = (await bob.post('/projects', { name: `Internal ${stamp}`, visibility: 'open' })).data;
  const task = (await ann.post('/tasks', { title: `Joint task ${stamp}`, project_id: group.id, responsible_id: ann.id })).data;
  const hidden = (await bob.post('/tasks', { title: `Internal task ${stamp}`, project_id: other.id, responsible_id: bob.id })).data;
  ok('only group owners invite guests to it', (await bob.post(`/projects/${group.id}/guests`, { email: guestEmail })).status === 403);
  r = await ann.post(`/projects/${group.id}/guests`, { email: guestEmail });
  ok('the same guest added to a group', r.status === 200 && r.data.invited === false && r.data.user_id === gilId, r.data);
  r = await gil.get('/projects');
  ok('they see that group only', r.data.some?.((p: any) => p.id === group.id) && !r.data.some((p: any) => p.id === other.id), r.data);
  ok('not other open groups', (await gil.get(`/projects/${other.id}`)).status === 404 && (await gil.post(`/projects/${other.id}/join`)).status === 404);
  ok('the group’s tasks', (await gil.get(`/tasks/${task.id}`)).status === 200);
  ok('not other groups’ tasks', (await gil.get(`/tasks/${hidden.id}`)).status === 404);
  r = await gil.get(`/tasks?q=${stamp}`);
  ok('in lists too', r.data.items?.some((t: any) => t.id === task.id) && !r.data.items.some((t: any) => t.id === hidden.id), r.data.items?.map((t: any) => t.title));
  r = await gil.get(`/projects/${group.id}`);
  ok('members show who is a guest', r.data.members.some((m: any) => m.user_id === gilId && m.is_guest === true), r.data.members);

  // ---- the feed ----
  await bob.post('/feed', { body: `Company news ${stamp}` });
  await ann.post('/feed', { body: `Group news ${stamp}`, project_id: group.id });
  r = await gil.get(`/feed?q=${encodeURIComponent(stamp)}`);
  ok('company posts are not theirs', !r.data.items.some((p: any) => p.body.includes('Company news')), r.data.items?.map((p: any) => p.body));
  r = await gil.get(`/feed?project_id=${group.id}`);
  ok('their group’s posts are', r.data.items.some((p: any) => p.body === `Group news ${stamp}`), r.data);
  ok('they post only in their groups', (await gil.post('/feed', { body: 'hi all' })).status === 403 && (await gil.post('/feed', { body: `From Gil ${stamp}`, project_id: group.id })).status === 200);

  // ---- boards ----
  const forAll = (await bob.post('/boards', { title: `For all ${stamp}` })).data;
  await bob.put(`/boards/${forAll.id}/access`, { grants: [{ kind: 'everyone', level: 'view' }] });
  const forGil = (await bob.post('/boards', { title: `For Gil ${stamp}` })).data;
  await bob.put(`/boards/${forGil.id}/access`, { grants: [{ kind: 'user', id: gilId, level: 'edit' }] });
  ok('boards shared with everyone are not for guests', (await gil.get(`/boards/${forAll.id}`)).status === 404);
  ok('boards shared with them by name are', (await gil.get(`/boards/${forGil.id}`)).data.level === 'edit');
  r = await gil.get('/boards?scope=shared');
  ok('and only those are listed', r.data.items.some((b: any) => b.id === forGil.id) && !r.data.items.some((b: any) => b.id === forAll.id), r.data.items?.map((b: any) => b.title));

  // ---- live events ----
  const gilWs = await gil.ws();
  const annWs = await ann.ws();
  await admin.post('/crm/lead/items', { title: `Lead ${stamp}` });
  await ann.post(`/im/chats/${chat.id}/messages`, { body: `Live ${stamp}` });
  await sleep(1500);
  ok('company-wide events never reach guests', !gilWs.events.some((e: any) => e.type === 'crm.changed') && annWs.events.some((e: any) => e.type === 'crm.changed'), gilWs.events.map((e: any) => e.type));
  ok('their own chat’s do', gilWs.events.some((e: any) => e.type === 'im.message'), gilWs.events.map((e: any) => e.type));
  gilWs.close();
  annWs.close();

  // ---- company-wide chats and the directory ----
  const everyone = (await admin.post('/im/chats', { title: `All hands ${stamp}`, audience: 'everyone' })).data;
  ok('(a company-wide chat to test with)', !!everyone.id, everyone);
  ok('company-wide chats leave guests out', !(await db.query('SELECT 1 FROM im_members WHERE chat_id = $1 AND user_id = $2', [everyone.id, gilId])).rowCount);
  r = await admin.get(`/users?q=${encodeURIComponent(`Out${stamp}`)}`);
  ok('the directory leaves guests out', !r.data.items.some((u: any) => u.id === gilId));
  r = await admin.get(`/users?with_guests=true&q=${encodeURIComponent(`Out${stamp}`)}`);
  ok('pickers find them, marked', r.data.items.some((u: any) => u.id === gilId && u.is_guest), r.data.items);
  r = await admin.get('/guests');
  const row = r.data.items.find((g: any) => g.id === gilId);
  ok('administrators see the guests and where they are', row?.chats?.includes(`Partners ${stamp}`) && row?.groups?.includes(`Joint ${stamp}`), row);
  ok('the guest list is for administrators', (await bob.get('/guests')).status === 403);
} finally {
  await db.end();
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
