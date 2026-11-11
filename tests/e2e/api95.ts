// The MCP server, spoken to by the official MCP client: a key sees the tools its scopes allow;
// tools act as the key's owner (their access rights, the key's scopes); bad arguments and
// refusals come back as errors; keys that stop working stop the server; browser pages of
// other sites are refused.
import { Client as McpClient } from '@modelcontextprotocol/sdk/client/index.js';
import { StreamableHTTPClientTransport } from '@modelcontextprotocol/sdk/client/streamableHttp.js';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const B = 'http://localhost:8080';
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const connect = async (key: string) => {
  const c = new McpClient({ name: 'nerp-test', version: '1.0.0' });
  await c.connect(new StreamableHTTPClientTransport(new URL(`${B}/api/mcp`), { requestInit: { headers: { authorization: `Bearer ${key}` } } }));
  return c;
};
const text = (r: any) => (r.content?.[0]?.text ?? '') as string;
const keys: string[] = [];

try {
  let r = await admin.post('/api-keys', { name: `Full ${stamp}`, scopes: ['crm', 'tasks', 'users:read'] });
  const full = r.data.secret as string;
  keys.push(r.data.key.id);
  r = await admin.post('/api-keys', { name: `Reader ${stamp}`, scopes: ['crm:read'] });
  const reader = r.data.secret as string;
  keys.push(r.data.key.id);

  const mcp = await connect(full);
  const info = mcp.getServerVersion();
  ok('the official MCP client connects', info?.name === 'nerp', info);
  const names = (await mcp.listTools()).tools.map((t) => t.name);
  ok('tools for the key\'s scopes', ['crm_search', 'crm_create', 'task_create', 'employees_search'].every((n) => names.includes(n)), names);
  const ro = await connect(reader);
  const roNames = (await ro.listTools()).tools.map((t) => t.name);
  ok('a read-only CRM key sees only CRM reading tools', roNames.includes('crm_search') && !roNames.includes('crm_create') && !roNames.includes('tasks_search'), roNames);

  // CRM.
  let res: any = await mcp.callTool({ name: 'crm_create', arguments: { type: 'lead', fields: { title: `From an assistant ${stamp}`, comments: 'Asked about pricing' } } });
  const lead = res.structuredContent;
  ok('it makes a CRM record', !res.isError && lead?.id && lead.title === `From an assistant ${stamp}`, res);
  res = await mcp.callTool({ name: 'crm_search', arguments: { type: 'lead', q: `assistant ${stamp}` } });
  ok('…finds it', !res.isError && res.structuredContent.items.some((i: any) => i.id === lead.id), text(res).slice(0, 300));
  res = await mcp.callTool({ name: 'crm_comment', arguments: { id: lead.id, body: 'Called back' } });
  ok('…comments on it', !res.isError, res);
  res = await mcp.callTool({ name: 'crm_add_activity', arguments: { item_id: lead.id, kind: 'call', subject: 'Follow up', deadline: new Date(Date.now() + 86400e3).toISOString() } });
  ok('…plans a call', !res.isError && res.structuredContent.kind === 'call', res);
  res = await ro.callTool({ name: 'crm_timeline', arguments: { id: lead.id } });
  ok('a reading key reads its history', !res.isError && text(res).includes('Called back'), text(res).slice(0, 200));
  await ro.callTool({ name: 'crm_create', arguments: { type: 'lead', fields: { title: 'x' } } }).then(() => ok('a reading key can\'t make records', false), () => ok('a reading key can\'t make records', true));
  res = await mcp.callTool({ name: 'crm_get', arguments: { type: 'lead', id: '00000000-0000-4000-8000-000000000000' } });
  ok('API refusals come back as tool errors', res.isError === true && /not found|404/i.test(text(res)), res);

  // Tasks and people.
  const me = (await admin.get('/auth/me')).data;
  res = await mcp.callTool({ name: 'task_create', arguments: { title: `Prepare the quote ${stamp}`, responsible_id: me.id ?? me.user?.id, priority: 'high' } });
  const task = res.structuredContent;
  ok('it sets a task', !res.isError && task?.id, res);
  res = await mcp.callTool({ name: 'task_set_status', arguments: { id: task.id, status: 'in_progress' } });
  ok('…moves it on', !res.isError && res.structuredContent.status === 'in_progress', res);
  res = await mcp.callTool({ name: 'task_update', arguments: { id: task.id, fields: { description: 'Two options, by Friday' } } });
  ok('…and changes it', !res.isError && res.structuredContent.description === 'Two options, by Friday', res);
  res = await mcp.callTool({ name: 'tasks_search', arguments: { q: `quote ${stamp}`, view: 'all' } });
  ok('…finds it', !res.isError && text(res).includes(task.id));
  res = await mcp.callTool({ name: 'employees_search', arguments: { q: 'admin' } });
  ok('it looks people up', !res.isError && text(res).includes('admin@acme.test'));

  // Arguments.
  await mcp.callTool({ name: 'crm_get', arguments: { type: 'lead' } }).then((x: any) => ok('missing arguments are refused', x.isError === true), () => ok('missing arguments are refused', true));
  await mcp.callTool({ name: 'crm_get', arguments: { type: '../users', id: lead.id } }).then((x: any) => ok('paths can\'t be steered', x.isError === true), () => ok('paths can\'t be steered', true));
  await mcp.callTool({ name: 'crm_search', arguments: { type: 'lead', sneaky: 1 } }).then((x: any) => ok('unknown arguments are refused', x.isError === true), () => ok('unknown arguments are refused', true));

  // The protocol around it.
  const raw = (body: unknown, headers: Record<string, string> = {}) => fetch(`${B}/api/mcp`, { method: 'POST', headers: { 'content-type': 'application/json', accept: 'application/json, text/event-stream', authorization: `Bearer ${full}`, ...headers }, body: JSON.stringify(body) });
  ok('notifications get 202', (await raw({ jsonrpc: '2.0', method: 'notifications/initialized' })).status === 202);
  const unknown = await (await raw({ jsonrpc: '2.0', id: 9, method: 'resources/list' })).json();
  ok('unknown methods are JSON-RPC errors', unknown.error?.code === -32601, unknown);
  ok('no key: 401', (await fetch(`${B}/api/mcp`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: '{}' })).status === 401);
  ok('a sign-in token isn\'t a key', (await raw({ jsonrpc: '2.0', id: 1, method: 'ping' }, { authorization: `Bearer ${admin.token}` })).status === 401);
  ok('pages of other sites are refused', (await raw({ jsonrpc: '2.0', id: 1, method: 'ping' }, { origin: 'https://evil.example' })).status === 403);
  ok('no event stream (GET 405)', (await fetch(`${B}/api/mcp`, { headers: { authorization: `Bearer ${full}` } })).status === 405);
  await admin.del(`/api-keys/${keys[1]}`);
  ok('a revoked key stops working', (await fetch(`${B}/api/mcp`, { method: 'POST', headers: { 'content-type': 'application/json', authorization: `Bearer ${reader}` }, body: JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'ping' }) })).status === 401);
  await mcp.close();
  await ro.close().catch(() => {});
  await admin.del(`/crm/lead/items/${lead.id}`);
  await admin.del(`/tasks/${task.id}`);
} finally {
  for (const k of keys) await admin.del(`/api-keys/${k}`).catch(() => {});
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
