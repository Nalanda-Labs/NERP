// Raising one's rights, endpoint by endpoint: every endpoint whose handler begins by
// requiring a permission (read from the backend's source) is called by an employee without
// any extra permission, and by one who has a permission for another module only (the
// knowledge base). Each must be refused. Then the admin-only areas are tried with
// plausible bodies, and nothing they hold may have changed.
import { admin, person, ok, check, finish, counts } from './lib.ts';
import { endpoints, call, fillPath, NIL, reach } from './sweep.ts';

const eve = await person('Eve');
const kay = await person('Kay', ['kb.manage']);
const before = (await admin.get('/settings')).data;
let n = 0;
for (const e of endpoints()) {
  if (!e.gated) continue;
  n++;
  const path = fillPath(e.path, [NIL]);
  const start = { title: 'pwned by eve', name: 'pwned by eve', body: 'pwned by eve' };
  // The body is completed until the handler is reached, so it is the permission that answers.
  const a = await reach(e.method, path, eve.token, start);
  check(`${e.method} ${e.path}: refused to an employee without the permission`, [401, 403, 404].includes(a.status), `${a.status} ${a.text.slice(0, 100)}`);
  if (!/^kb\.rs$/.test(e.route.file)) {
    const b = await reach(e.method, path, kay.token, start);
    check(`${e.method} ${e.path}: refused to a manager of another module`, [401, 403, 404].includes(b.status), `${b.status} ${b.text.slice(0, 100)}`);
  }
}
ok(`(${n} permission-gated endpoints tried)`, n > 150);

// ---- admin areas with plausible bodies ----
const tries: [string, string, unknown][] = [
  ['PUT', '/settings', { company_name: 'Pwned', ip_allow: ['203.0.113.1'], public_links: false, require_two_factor: true }],
  ['PATCH', '/settings', { company_name: 'Pwned' }],
  ['POST', '/roles', { code: 'pwn', name: 'Pwn', permissions: ['*'] }],
  ['PUT', `/users/${eve.id}/roles`, { role_ids: [] }],
  ['POST', '/users', { email: `pwn.${Date.now()}@acme.test`, first_name: 'P', last_name: 'W', password: 'password123' }],
  ['POST', '/join-links', { note: 'pwn' }],
  ['POST', '/api-keys', { name: 'pwn', scopes: ['crm'] }],
  ['POST', '/webhooks', { name: 'pwn', url: 'https://example.com', events: ['task.created'] }],
  ['PUT', '/settings/ui-default', { hidden: ['crm'] }],
  ['POST', '/system/backups', {}],
  ['POST', '/system/storage/cleanup', {}],
  ['PUT', '/auth/providers/google', { enabled: true, client_id: 'x', client_secret: 'y' }],
  ['POST', '/directory/ldap/sync', {}],
  ['PUT', '/shop/settings', { enabled: false }],
  ['POST', '/crm/doc-templates', { name: 'x', engine: 'latex', source: '\\directlua{os.exit()}' }],
  ['POST', '/accounting/setup', {}],
  ['PATCH', '/hr/settings', { workday_hours: 1 }],
  ['POST', '/guests', { email: 'x@y.z' }],
];
for (const [m, p, b] of tries) {
  const r = await call(m, p, { token: eve.token, body: JSON.stringify(b) });
  check(`${m} ${p}: an admin area refuses an employee`, [401, 403, 404, 405].includes(r.status), r.status);
}
const after = (await admin.get('/settings')).data;
ok('the company settings did not change', after.company_name === before.company_name && JSON.stringify(after.ip_allow) === JSON.stringify(before.ip_allow));
const me = (await eve.get('/auth/me')).data;
ok('and the employee still has no permissions', (me.permissions ?? []).length === 0 || (me.permissions ?? []).every((p: string) => !p.includes('manage') && p !== '*'), me.permissions);
console.log(`checks: ${JSON.stringify(counts())}`);
await finish();
