// Public API: API keys (scopes, owner's rights, revocation, deactivation), outgoing event
// webhooks (delivery, signature, retries, test, redeliver) and the API reference.
import http from 'node:http';
import { createHmac } from 'node:crypto';
import { cleanupRun } from './cleanup.ts';
import { Client } from './lib.ts';
const B = 'http://localhost:8080/api';
let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 700)); };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const stamp = Date.now().toString(36);
const admin = new Client(); admin.id = (await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' })).data.user.id;
async function mk(name) {
  const email = `${name}.${stamp}@acme.test`;
  const r = await admin.post('/users', { email, password: 'password123', first_name: name, last_name: 'Api' });
  const c = new Client(); await c.post('/auth/login', { email, password: 'password123' }); c.id = r.data.id; return c;
}
const sam = await mk('sam');
const waitFor = async (fn, ms = 15000) => { const end = Date.now() + ms; let v; while (Date.now() < end) { v = await fn(); if (v) return v; await sleep(250); } return v; };

// ---- a webhook receiver ----
const hits: any[] = [];
const server = http.createServer((req, res) => {
  let body = ''; req.on('data', (c) => (body += c));
  req.on('end', () => {
    hits.push({ url: req.url!, headers: req.headers, body, json: (() => { try { return JSON.parse(body); } catch { return null; } })() });
    const code = req.url!.startsWith('/fail500') ? 503 : req.url!.startsWith('/fail400') ? 400 : 200;
    res.writeHead(code); res.end(code === 200 ? 'thanks' : 'nope');
  });
});
await new Promise<void>((r) => server.listen(0, '127.0.0.1', () => r()));
const hookUrl = `http://127.0.0.1:${(server.address() as { port: number }).port}`;

// ---- API keys ----
ok('employees need the "Use the API" permission', (await sam.post('/api-keys', { name: `k ${stamp}`, scopes: ['crm'] })).status === 403);
let r: any = await admin.get('/api-keys');
ok('scopes are listed', r.data.scopes?.some((s) => s.code === 'crm:read') && r.data.can_create === true, r.data);
ok('bad scope rejected', (await admin.post('/api-keys', { name: `bad ${stamp}`, scopes: ['everything'] })).status === 400);
ok('bad expiry rejected', (await admin.post('/api-keys', { name: `bad ${stamp}`, scopes: ['crm'], expires_in_days: 9999 })).status === 400);
r = await admin.post('/api-keys', { name: `Reader ${stamp}`, scopes: ['crm:read', 'users:read'], expires_in_days: 30 });
const reader = r.data;
ok('key created; the secret shows once', r.status === 200 && reader.secret?.startsWith('nerp_') && reader.key.prefix === reader.secret.slice(0, 11) && reader.key.expires_at, r.data);
ok('lists show the prefix, never the secret', !JSON.stringify((await admin.get('/api-keys')).data).includes(reader.secret));
const rk = new Client(reader.secret);
ok('read scope: list deals', (await rk.get('/crm/deal/items?limit=1')).status === 200);
ok('read scope: cannot create', (await rk.post('/crm/deal/items', { title: `nope ${stamp}` })).status === 403);
ok('no tasks scope: no tasks', (await rk.get('/tasks')).status === 403);
ok('users:read: list employees', (await rk.get('/users?limit=1')).status === 200);
ok('users:read: cannot add employees', (await rk.post('/users', { email: `x.${stamp}@acme.test` })).status === 403);
ok('keys cannot manage keys', (await rk.get('/api-keys')).status === 403);
ok('keys cannot reach other areas', (await rk.get('/settings')).status === 403 && (await rk.get('/notifications')).status === 403 && (await rk.get('/auth/sessions')).status === 403);
ok('unknown key refused', (await new Client('nerp_' + 'x'.repeat(40)).get('/crm/deal/items')).status === 401);
await waitFor(async () => (await admin.get('/api-keys')).data.items.find((k) => k.id === reader.key.id)?.last_used_at);
ok('last use recorded', !!(await admin.get('/api-keys')).data.items.find((k) => k.id === reader.key.id)?.last_used_at);

r = await admin.post('/api-keys', { name: `Writer ${stamp}`, scopes: ['crm', 'tasks'] });
const wk = new Client(r.data.secret);
r = await wk.post('/crm/deal/items', { title: `Api deal ${stamp}`, opportunity: '300' });
ok('full scope: create a deal as the key\'s owner', r.status === 200 && r.data.created_by === admin.id, r.data);
const deal = r.data;

// An employee with the permission, through a role.
const role = (await admin.post('/roles', { code: `api_${stamp}`, name: `API users ${stamp}`, permissions: ['api.keys'] })).data;
const empRole = (await admin.get('/roles')).data.find((x) => x.code === 'employee');
await admin.put(`/users/${sam.id}/roles`, { role_ids: [empRole.id, role.id] });
r = await sam.post('/api-keys', { name: `Sam key ${stamp}`, scopes: ['crm:read'] });
ok('employee with the permission makes a key', r.status === 200, r.data);
const sk = new Client(r.data.secret), samKey = r.data.key;
ok('it acts as the employee', (await sk.get(`/crm/deal/items/${deal.id}`)).status === 200);
ok('employee sees only their own keys', (await sam.get('/api-keys')).data.items.every((k) => k.user_id === sam.id) && (await sam.get('/api-keys?all=true')).status === 403);
ok('admin sees everyone\'s keys', (await admin.get('/api-keys?all=true')).data.items.some((k) => k.id === samKey.id));
await admin.put(`/users/${sam.id}/roles`, { role_ids: [empRole.id] });
ok('losing the permission stops the key', (await sk.get('/crm/deal/items?limit=1')).status === 401);
await admin.put(`/users/${sam.id}/roles`, { role_ids: [empRole.id, role.id] });
ok('getting it back restores it', (await sk.get('/crm/deal/items?limit=1')).status === 200);
await admin.patch(`/users/${sam.id}`, { is_active: false });
ok('deactivated owner: key refused', (await sk.get('/crm/deal/items?limit=1')).status === 401);
await admin.patch(`/users/${sam.id}`, { is_active: true });
await sam.post('/auth/login', { email: `sam.${stamp}@acme.test`, password: 'password123' }); // deactivation ended sam's sessions
ok('admin revokes someone\'s key', (await admin.del(`/api-keys/${samKey.id}`)).status === 200);
ok('revoked key refused at once', (await sk.get('/crm/deal/items?limit=1')).status === 401);
ok('employee cannot revoke others\' keys', (await sam.del(`/api-keys/${reader.key.id}`)).status === 404);

// ---- webhooks ----
ok('employees cannot manage webhooks', (await sam.get('/webhooks')).status === 403);
r = await admin.get('/webhooks/events');
ok('event list', r.data.items.some((e) => e.code === 'crm.item.stage_changed') && r.data.items.some((e) => e.code === 'task.completed'));
ok('bad URL rejected', (await admin.post('/webhooks', { name: `w ${stamp}`, url: 'ftp://x', events: ['task.created'] })).status === 400);
ok('bad event rejected', (await admin.post('/webhooks', { name: `w ${stamp}`, url: hookUrl, events: ['everything'] })).status === 400);
r = await admin.post('/webhooks', { name: `Sync ${stamp}`, url: `${hookUrl}/events`, events: ['crm.item.created', 'crm.item.updated', 'crm.item.stage_changed', 'task.created', 'task.completed'] });
const sub = r.data;
ok('webhook created with a secret', r.status === 200 && sub.secret?.length >= 20, r.data);
const signed = (h) => h.headers['x-nerp-signature'] === 'sha256=' + createHmac('sha256', sub.secret).update(h.body).digest('hex');
const hit = (pred) => waitFor(() => hits.find(pred));

r = await wk.post('/crm/deal/items', { title: `Hooked ${stamp}`, opportunity: '1000' });
const hd = r.data;
let h = await hit((x) => x.json?.event === 'crm.item.created' && x.json.data.id === hd.id);
ok('created event delivered, signed', h && signed(h) && h.headers['x-nerp-event'] === 'crm.item.created' && h.json.data.type === 'deal' && h.json.data.title === `Hooked ${stamp}` && h.json.data.url.endsWith(`/crm/deal/${hd.id}`), h?.json);
await wk.patch(`/crm/deal/items/${hd.id}`, { opportunity: '1500' });
h = await hit((x) => x.json?.event === 'crm.item.updated' && x.json.data.id === hd.id);
ok('updated event lists the changes', h && h.json.data.changes.some((c) => c.field === 'opportunity' && +c.to === 1500), h?.json);
const stages = (await admin.get('/crm/config')).data.types.find((t) => t.code === 'deal').categories.find((c) => c.id === hd.category_id).stages;
await wk.patch(`/crm/deal/items/${hd.id}`, { stage_id: stages[1].id });
h = await hit((x) => x.json?.event === 'crm.item.stage_changed' && x.json.data.id === hd.id);
ok('stage event with the previous stage', h && h.json.data.stage_id === stages[1].id && h.json.data.previous_stage_id === stages[0].id, h?.json);
const task = (await wk.post('/tasks', { title: `Api task ${stamp}`, crm_item_ids: [hd.id] })).data;
ok('task created through a key', !!task.id, task);
ok('task.created delivered', !!(await hit((x) => x.json?.event === 'task.created' && x.json.data.id === task.id)));
await wk.post(`/tasks/${task.id}/status`, { status: 'done' });
ok('task.completed delivered', !!(await hit((x) => x.json?.event === 'task.completed' && x.json.data.id === task.id)));
await wk.del(`/crm/deal/items/${hd.id}`);
await sleep(1500);
ok('events not subscribed to are not sent', !hits.some((x) => x.json?.event === 'crm.item.deleted'));
r = await admin.get(`/webhooks/${sub.id}/deliveries`);
ok('delivery log', r.data.items.filter((d) => d.status === 'done').length >= 5 && r.data.items[0].response_status === 200, r.data.items.map((d) => [d.event, d.status]));
ok('stats on the list', (await admin.get('/webhooks')).data.items.find((w) => w.id === sub.id)?.delivered_24h >= 5);

r = await admin.post(`/webhooks/${sub.id}/test`);
ok('test ping', r.status === 200 && !!(await hit((x) => x.json?.event === 'ping' && x.json.id === r.data.delivery_id)));
const firstPing = r.data.delivery_id;
r = await admin.post(`/webhook-deliveries/${firstPing}/redeliver`);
ok('redeliver sends again as a new delivery', r.status === 200 && r.data.delivery_id !== firstPing && !!(await hit((x) => x.json?.id === r.data.delivery_id)));
const oldSecret = sub.secret;
r = await admin.patch(`/webhooks/${sub.id}`, { rotate_secret: true, is_active: false });
ok('secret rotated; turned off', r.data.secret !== oldSecret && r.data.is_active === false);
const before = hits.length;
await wk.post('/crm/deal/items', { title: `Quiet ${stamp}` });
await sleep(1500);
ok('inactive webhook gets nothing', hits.length === before);

// Failing receivers.
const w5 = (await admin.post('/webhooks', { name: `Down ${stamp}`, url: `${hookUrl}/fail500`, events: ['task.created'] })).data;
const w4 = (await admin.post('/webhooks', { name: `Wrong ${stamp}`, url: `${hookUrl}/fail400`, events: ['task.created'] })).data;
await admin.post(`/webhooks/${w5.id}/test`); await admin.post(`/webhooks/${w4.id}/test`);
const d5 = await waitFor(async () => (await admin.get(`/webhooks/${w5.id}/deliveries`)).data.items.find((d) => d.attempts >= 1));
ok('5xx is retried later', d5?.status === 'pending' && d5.response_status === 503 && new Date(d5.next_attempt_at) > new Date(Date.now() + 30000), d5);
const d4 = await waitFor(async () => (await admin.get(`/webhooks/${w4.id}/deliveries`)).data.items.find((d) => d.attempts >= 1));
ok('4xx fails at once', d4?.status === 'failed' && d4.response_status === 400 && !d4.next_attempt_at, d4);
ok('filter deliveries by status', (await admin.get(`/webhooks/${w4.id}/deliveries?status=failed`)).data.items.length === 1);
ok('delete a webhook', (await admin.del(`/webhooks/${w5.id}`)).status === 200 && (await admin.get(`/webhooks/${w5.id}/deliveries`)).status === 404);

// ---- reference ----
r = await new Client().get('/openapi.json');
ok('API reference', r.status === 200 && r.data.openapi === '3.1.0' && r.data.paths['/crm/{type}/items'] && r.data.webhooks['crm.item.updated'], Object.keys(r.data));

server.close();
await admin.del(`/roles/${role.id}`).catch(() => {});
await cleanupRun(stamp);
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
