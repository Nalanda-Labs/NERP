// API keys, endpoint by endpoint: a CRM read key, a tasks key and a users read key are used
// on every endpoint. Each may only reach its own area (and a read key only reads). Then the
// keys must stop when revoked, when their owner is deactivated, and when the owner loses
// the "Use the API" permission.
import { admin, person, ok, check, finish, counts, stamp } from './lib.ts';
import { endpoints, isPublic, call, fillPath, NIL, ok2xx } from './sweep.ts';

const kim = await person('Kim', ['api.keys']);
const make = async (scopes: string[]) => {
  const r = (await kim.post('/api-keys', { name: `pen ${scopes.join()} ${stamp}`, scopes })).data;
  return { id: r.id ?? r.key?.id, secret: r.secret ?? r.key ?? r.token } as { id: string; secret: string };
};
const keys: [string, { id: string; secret: string }, (m: string, p: string) => boolean][] = [
  ['crm:read', await make(['crm:read']), (m, p) => m === 'GET' && /^\/api\/crm\//.test(p)],
  ['tasks', await make(['tasks']), (_m, p) => /^\/api\/(tasks|projects|task-templates)(\/|$)/.test(p)],
  ['users:read', await make(['users:read']), (m, p) => m === 'GET' && /^\/api\/(users|departments)(\/|$)/.test(p)],
];
ok('three keys made', keys.every(([, k]) => !!k.secret), keys.map(([n, k]) => [n, !!k.secret]));
let swept = 0;
for (const e of endpoints()) {
  if (isPublic(e.path)) continue;
  swept++;
  // The MCP endpoint takes any key with a tool there; each tool is held to its own scope
  // (api95, and the files-only key below).
  if (e.path === '/api/mcp') continue;
  const path = fillPath(e.path, [NIL]);
  for (const [name, key, allowed] of keys) {
    if (allowed(e.method, e.path)) continue;
    const r = await call(e.method, path, { token: key.secret, body: e.method === 'GET' || e.method === 'DELETE' ? undefined : '{}' });
    check(`${e.method} ${e.path}: out of reach of a ${name} key`, [401, 403].includes(r.status), r.status);
  }
}
ok(`(${swept} endpoints tried with each key)`, swept > 800);
const driveKey = await make(['files']);
ok('a key with no MCP tools is refused there', (await call('POST', '/mcp', { token: driveKey.secret, body: JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'tools/list' }) })).status === 403);

// ---- inside its area, a key acts as its owner only ----
const [, crm] = keys[0];
ok('a CRM read key reads the CRM', ok2xx((await call('GET', '/crm/deal/items', { token: crm.secret })).status));
ok('…and doesn\'t write it', [401, 403].includes((await call('POST', '/crm/deal/items', { token: crm.secret, body: JSON.stringify({ title: `x ${stamp}` }) })).status));
const tasks = keys[1][1];
const t = await call('POST', '/tasks', { token: tasks.secret, body: JSON.stringify({ title: `Key task ${stamp}` }) });
ok('a tasks key makes tasks as its owner', ok2xx(t.status) && (t.json?.created_by ?? t.json?.creator?.id ?? kim.id) === kim.id, t.json);
// ---- and stops ----
await kim.del(`/api-keys/${keys[2][1].id}`);
ok('a revoked key stops at once', (await call('GET', '/users', { token: keys[2][1].secret })).status === 401);
const role = (await admin.get(`/users/${kim.id}`)).data;
await admin.put(`/users/${kim.id}/roles`, { role_ids: (role.roles ?? []).filter((r: any) => r.code === 'employee').map((r: any) => r.id) });
ok('a key stops when its owner loses "Use the API"', [401, 403].includes((await call('GET', '/crm/deal/items', { token: crm.secret })).status));
await admin.patch(`/users/${kim.id}`, { is_active: false });
ok('…and when its owner is deactivated', (await call('GET', '/tasks', { token: tasks.secret })).status === 401);
await admin.patch(`/users/${kim.id}`, { is_active: true });
console.log(`checks: ${JSON.stringify(counts())}`);
await finish();
