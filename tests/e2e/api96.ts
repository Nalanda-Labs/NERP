// Everyone's mailboxes: only people with "Manage everyone's mailboxes" see them all; mailboxes
// connected for employees in bulk (each checked first; failures reported, the rest kept; the
// employee told); pausing, syncing and disconnecting; and mailboxes shared by role and by
// department (with sub-departments). Uses the local Dovecot test server.
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const roles = (await admin.get('/roles')).data;
const employee = roles.find((r: any) => r.code === 'employee');
const mk = async (first: string, roleIds: string[] = []) => {
  const email = `${first.toLowerCase()}.${stamp}@acme.test`;
  const u = (await admin.post('/users', { email, password: 'password123', first_name: first, last_name: `Mb${stamp}` })).data;
  if (roleIds.length) await admin.put(`/users/${u.id}/roles`, { role_ids: [employee.id, ...roleIds] });
  const c = new Client();
  await c.post('/auth/login', { email, password: 'password123' });
  c.id = u.id;
  return Object.assign(c, { email });
};
const server = { imap_host: '127.0.0.1', imap_port: 143, imap_security: 'none', smtp_host: '127.0.0.1', smtp_port: 1025, smtp_security: 'starttls', smtp_auth: false, allow_invalid_certs: true };
const made: string[] = [];
const roleIds: string[] = [];

try {
  const mailAdmin = (await admin.post('/roles', { code: `mailadm_${stamp}`, name: `Mail admins ${stamp}`, permissions: ['mail.admin'] })).data;
  const salesRole = (await admin.post('/roles', { code: `sales_${stamp}`, name: `Sales ${stamp}`, permissions: [] })).data;
  roleIds.push(mailAdmin.id, salesRole.id);
  const ann = await mk('Ann', [mailAdmin.id]), bob = await mk('Bob'), cat = await mk('Cat'), dan = await mk('Dan', [salesRole.id]), eve = await mk('Eve');
  const dept = (await admin.post('/departments', { name: `Support ${stamp}` })).data;
  const sub = (await admin.post('/departments', { name: `Support desk ${stamp}`, parent_id: dept.id })).data;
  await admin.post(`/departments/${sub.id}/members`, { user_id: cat.id });

  ok('others can\'t see everyone\'s mailboxes', (await bob.get('/admin/mailboxes')).status === 403);
  ok('…nor connect them', (await bob.post('/admin/mailboxes/bulk', { server, accounts: [{ user_email: bob.email, password: 'x' }] })).status === 403);
  let r = await ann.post('/admin/mailboxes/bulk', {
    server,
    sync_days: 1,
    accounts: [
      { user_email: bob.email, email: 'sales@nerp.test', password: 'nerptest' },
      { user_email: cat.email, email: 'support@nerp.test', password: 'wrong' },
      { user_email: `nobody.${stamp}@acme.test`, password: 'x' },
    ],
  });
  ok('bulk connecting: one connected, two reported', r.status === 200 && r.data.connected === 1 && r.data.failed === 2, r.data);
  ok('…why each failed', /could not connect/.test(r.data.results[1].error) && /no such employee/.test(r.data.results[2].error), r.data.results);
  const mb = r.data.results[0].id as string;
  made.push(mb);
  ok('the employee owns it', (await bob.get('/crm/mailboxes')).data.items.some((m: any) => m.id === mb && m.is_owner));
  ok('…and is told', (await bob.get('/notifications?limit=10')).data.items.some((n: any) => n.kind === 'crm.mailbox' && n.title.includes('sales@nerp.test')));
  r = await ann.post('/admin/mailboxes/bulk', { server, accounts: [{ user_email: bob.email, email: 'sales@nerp.test', password: 'nerptest' }] });
  ok('the same mailbox isn\'t connected twice', r.data.connected === 0 && /already connected/.test(r.data.results[0].error), r.data);

  r = await ann.get('/admin/mailboxes');
  const row = r.data.items.find((m: any) => m.id === mb);
  ok('the admin list: owner, address, state', row?.owner.name === `Bob Mb${stamp}` && row.email === 'sales@nerp.test' && ['waiting', 'ok'].includes(row.status), row);
  ok('…and who has none yet', r.data.without_mailbox.some((u: any) => u.id === cat.id) && !r.data.without_mailbox.some((u: any) => u.id === bob.id));
  await ann.patch(`/admin/mailboxes/${mb}`, { sync_enabled: false });
  ok('pausing', (await ann.get('/admin/mailboxes')).data.items.find((m: any) => m.id === mb).status === 'paused');
  await ann.patch(`/admin/mailboxes/${mb}`, { sync_enabled: true });
  ok('…and syncing now', (await ann.post(`/admin/mailboxes/${mb}/sync`)).status === 200);

  // Shared by role and department.
  ok('Dan and Cat can\'t use Bob\'s mailbox yet', (await dan.post(`/crm/mailboxes/${mb}/sync`)).status === 404 && (await cat.post(`/crm/mailboxes/${mb}/sync`)).status === 404);
  r = await bob.patch(`/crm/mailboxes/${mb}`, { shared_roles: [salesRole.id], shared_departments: [dept.id] });
  ok('Bob shares it with a role and a department', r.status === 200 && r.data.shared_roles.includes(salesRole.id), r.data);
  ok('everyone in the role uses it', (await dan.get('/crm/mailboxes')).data.items.some((m: any) => m.id === mb && !m.is_owner) && (await dan.post(`/crm/mailboxes/${mb}/sync`)).status === 200);
  ok('…and everyone in the department\'s sub-departments', (await cat.get('/crm/mailboxes')).data.items.some((m: any) => m.id === mb));
  ok('…not others', !(await eve.get('/crm/mailboxes')).data.items.some((m: any) => m.id === mb) && (await eve.post(`/crm/mailboxes/${mb}/sync`)).status === 404);
  ok('…who can\'t change it', (await dan.patch(`/crm/mailboxes/${mb}`, { shared_roles: [] })).status === 404);
  ok('the admin list shows the sharing', (await ann.get('/admin/mailboxes')).data.items.find((m: any) => m.id === mb).shared.roles === 1);

  r = await ann.del(`/admin/mailboxes/${mb}`);
  ok('disconnecting', r.status === 200 && !(await bob.get('/crm/mailboxes')).data.items.some((m: any) => m.id === mb));
  ok('…its owner is told', (await bob.get('/notifications?limit=10')).data.items.some((n: any) => n.title.includes('was disconnected')));
} finally {
  for (const id of made) await admin.del(`/crm/mailboxes/${id}`).catch(() => {});
  await cleanupRun(stamp);
  for (const id of roleIds) await admin.del(`/roles/${id}`).catch(() => {});
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
