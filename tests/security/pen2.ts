// Permissions: an employee cannot raise their own rights or reach admin-only actions, and
// cannot see or change another employee's private things through ANY route (every route
// with an id is tried with each of the other person's ids and every method). Every route
// also gets odd input from an employee without the server failing (no 500s).
import { routes, fill, type Method } from './routes.ts';
import { admin, person, raw, ok, finish, BASE } from './lib.ts';

const ann = await person('Ann');
const bob = await person('Bob');
const cat = await person('Cat');

// ---- raising one's own rights ----
const roles = (await admin.get('/roles')).data;
const adminRole = roles.find((r: any) => r.code === 'admin');
ok('cannot give oneself the admin role', (await bob.put(`/users/${bob.id}/roles`, { role_ids: [adminRole.id] })).status === 403);
ok('cannot make a role', (await bob.post('/roles', { code: 'mine', name: 'Mine', permissions: ['*'] })).status === 403);
ok('cannot edit their own account as an admin would', (await bob.patch(`/users/${bob.id}`, { email: 'boss@acme.test', is_active: true })).status === 403);
let r = await bob.patch('/profile', { first_name: 'Bob', permissions: ['*'], role_ids: [adminRole.id], is_admin: true, email: 'boss@acme.test', is_active: true });
const me = (await bob.get('/auth/me')).data;
const meUser = me.user ?? me;
ok('extra fields on the profile change nothing', !JSON.stringify(me).includes('"*"') && meUser.email === bob.email, me);
ok('…and the admin role was not added', !(await admin.get(`/users/${bob.id}`)).data.roles?.some((x: any) => x.code === 'admin'));
const adminOnly: [string, string, unknown?][] = [
  ['POST', '/users', { email: 'x@acme.test', first_name: 'X', last_name: 'Y', password: 'password123' }],
  ['GET', '/audit'], ['PUT', '/settings', { company_name: 'Pwned' }], ['GET', '/admin/jobs'], ['GET', '/admin/system'],
  ['POST', '/api-keys', { name: 'k', scopes: ['crm'] }], ['POST', '/webhooks', { name: 'w', url: 'https://example.com', events: ['task.created'] }],
  ['PATCH', '/hr/settings', { workday_hours: 1 }], ['PUT', '/support/settings', { auto_close_days: 1 }], ['POST', '/support/dictionary', { kind: 'category', name: 'x' }],
  ['POST', '/kb/spaces', { name: 'x' }], ['POST', '/training/courses', { title: 'x' }], ['POST', '/booking/resources', { name: 'x', kind: 'room' }],
  ['POST', `/users/${ann.id}/password`, { password: 'password999' }], ['POST', `/users/${ann.id}/sessions/revoke`], ['GET', `/users/${ann.id}/sessions`],
  ['POST', '/crm/doc-templates', { name: 'x', engine: 'typst', source: 'x' }], ['DELETE', `/support/tickets/00000000-0000-4000-8000-000000000000`],
];
const allowed: string[] = [];
for (const [m, p, body] of adminOnly) {
  const res = await bob.req(m, p, body ?? (m === 'GET' || m === 'DELETE' ? undefined : {}));
  if (![401, 403, 404].includes((res as any).status)) allowed.push(`${m} ${p} → ${(res as any).status}`);
}
ok('admin-only actions are refused to employees', allowed.length === 0, allowed);
ok('the company name did not change', (await admin.get('/settings')).data.company_name !== 'Pwned');

// ---- Ann's private things ----
const fd = new FormData();
fd.append('file', new Blob(['Ann private notes']), 'ann-notes.txt');
const file = (await ann.post('/files', fd)).data;
const task = (await ann.post('/tasks', { title: 'Ann private task' })).data;
const node = (await ann.post('/drive/files', { file_id: (await ann.post('/files', (() => { const f = new FormData(); f.append('file', new Blob(['drive secret']), 'drive.txt'); return f; })())).data.id, name: 'drive.txt' })).data;
const chat = (await ann.post('/im/chats', { title: 'Ann and Cat', members: [cat.id] })).data;
await ann.post(`/im/chats/${chat.id}/messages`, { body: 'just between us' });
const start = new Date(Date.now() + 86400000);
const event = (await ann.post('/calendar/events', { title: 'Ann private event', timezone: 'UTC', start: start.toISOString(), end: new Date(+start + 3600000).toISOString(), is_private: true })).data;
const ticket = (await ann.post('/support/tickets', { title: 'Ann private ticket', body: 'help' })).data;
const ids: Record<string, string | undefined> = { file: file?.id, task: task?.id, drive: node?.id ?? node?.node?.id, chat: chat?.id, event: event?.id, ticket: ticket?.id };
ok('Ann has private things to protect', Object.values(ids).every(Boolean), ids);

// ---- Bob tries every route with each of Ann's ids ----
// Saved CRM filters are per person: deleting someone else's id deletes nothing (and says ok).
const SKIP = /^\/api\/(auth|realtime|public|setup)\b|\/api\/users\/\{|\/api\/files$|^\/api\/crm\/filters\/\{id\}$/;
const leaks: string[] = [];
const bodyFor = (m: Method) => (m === 'GET' || m === 'DELETE' ? undefined : { title: 'pwned by bob', name: 'pwned by bob', body: 'pwned by bob', members: [bob.id], user_id: bob.id });
for (const route of routes()) {
  const params = route.path.match(/\{[^}]+\}/g) ?? [];
  if (params.length !== 1 || SKIP.test(route.path)) continue;
  for (const [what, id] of Object.entries(ids)) {
    for (const m of route.methods) {
      const res = (await bob.req(m, route.path.replace('/api', '').replace(params[0], id!), bodyFor(m))) as any;
      if (res.status >= 200 && res.status < 300) leaks.push(`${m} ${route.path} with Ann's ${what} → ${res.status} ${JSON.stringify(res.data).slice(0, 120)}`);
    }
  }
}
ok("Bob reaches none of Ann's private things through any route", leaks.length === 0, leaks);

// ---- Ann's things are as she left them ----
ok('her task is untouched', (await ann.get(`/tasks/${ids.task}`)).data?.title === 'Ann private task');
ok('her file is still hers', (await ann.req('GET', `/files/${ids.file}/content`, undefined, true) as Response).status === 200);
ok('her chat has no Bob', !(await ann.get(`/im/chats/${ids.chat}`)).data?.members?.some?.((x: any) => (x.id ?? x.user_id) === bob.id));
ok('her event is untouched', (await ann.get(`/calendar/events/${ids.event}`)).data?.title === 'Ann private event');

// ---- odd input on every route: no server errors ----
const errors: string[] = [];
for (const route of routes()) {
  if (/^\/api\/(auth|realtime|setup)\b/.test(route.path)) continue;
  for (const m of route.methods) {
    for (const body of m === 'GET' || m === 'DELETE' ? [undefined] : ['{}', '[]', '"x"', 'null', '{"id": {"$ne": 1}}', '{bad json']) {
      const res = await raw(m, fill(route.path), { base: BASE, headers: { authorization: `Bearer ${bob.token}`, ...(body ? { 'content-type': 'application/json' } : {}) }, body });
      if (res.status >= 500) errors.push(`${m} ${route.path} ${body ?? ''} → ${res.status} ${res.text.slice(0, 100)}`);
    }
  }
}
ok('no route fails on odd input (no 500s)', errors.length === 0, errors);

await finish();
