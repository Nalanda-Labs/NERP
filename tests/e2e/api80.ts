// CRM custom fields seen only by chosen people and departments: the others don't get the
// field, its values (record, list, board, exports, documents, e-mail templates), its changes in
// the history, can't filter by it or write it, and aren't asked to fill it in when it's required.
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
// A role that works on every deal, so the three colleagues share them.
const roles = (await admin.get('/roles')).data;
const dealRole = (await admin.post('/roles', { code: `deals_${stamp}`, name: `All deals ${stamp}`, permissions: [] })).data;
await admin.put(`/crm/permissions/${dealRole.id}`, { entries: ['read', 'add', 'write', 'export'].map((action) => ({ entity_type: 2, action, scope: 'all' })) });
const user = async (first: string) => {
  const email = `${first.toLowerCase()}.${stamp}@acme.test`;
  const u = (await admin.post('/users', { email, password: 'password123', first_name: first, last_name: `Fv${stamp}` })).data;
  await admin.put(`/users/${u.id}/roles`, { role_ids: [roles.find((r: any) => r.code === 'employee').id, dealRole.id] });
  const c = new Client();
  await c.post('/auth/login', { email, password: 'password123' });
  c.id = u.id;
  return c;
};
const fields: string[] = [];

try {
  const ann = await user('Ann'), bob = await user('Bob'), cat = await user('Cat');
  const dept = (await admin.post('/departments', { name: `Finance ${stamp}` })).data;
  const sub = (await admin.post('/departments', { name: `Payroll ${stamp}`, parent_id: dept.id })).data;
  await admin.post(`/departments/${sub.id}/members`, { user_id: cat.id });

  // A margin field only Ann and the finance department (with its sub-departments) see,
  // required, and an ordinary one.
  let r = await admin.post('/crm/deal/fields', { label: `Margin ${stamp}`, kind: 'number', is_required: true, visible_user_ids: [ann.id], visible_department_ids: [dept.id] });
  ok('a field for chosen people', r.status === 200 && r.data.visible_user_ids?.includes(ann.id), r.data);
  const margin = r.data;
  fields.push(margin.id);
  const region = (await admin.post('/crm/deal/fields', { label: `Region ${stamp}`, kind: 'string' })).data;
  fields.push(region.id);
  ok('guests and unknown departments are refused', (await admin.post('/crm/deal/fields', { label: `X ${stamp}`, kind: 'string', visible_department_ids: ['00000000-0000-4000-8000-000000000000'] })).status === 400);

  const m = margin.code, g = region.code;
  r = await bob.post('/crm/deal/items', { title: `Deal ${stamp}`, uf: { [g]: 'North' } });
  ok('someone who can\'t see a required field isn\'t asked for it', r.status === 200, r.data);
  const deal = r.data.id;
  r = await ann.post('/crm/deal/items', { title: `Deal two ${stamp}` });
  ok('…but those who see it are', r.status === 400 && /Margin/.test(JSON.stringify(r.data)), r.data);
  r = await ann.patch(`/crm/deal/items/${deal}`, { uf: { [m]: 42 } });
  ok('the chosen person writes it', r.status === 200 && r.data.uf[m] === 42, r.data);
  ok('…and the record shows it to them', (await ann.get(`/crm/deal/items/${deal}`)).data.uf[m] === 42);
  ok('a member of a sub-department sees it', (await cat.get(`/crm/deal/items/${deal}`)).data.uf[m] === 42);
  ok('CRM admins see it', (await admin.get(`/crm/deal/items/${deal}`)).data.uf[m] === 42);

  const det = (await bob.get(`/crm/deal/items/${deal}`)).data;
  ok('the others don\'t see its value', !(m in det.uf) && det.uf[g] === 'North', det.uf);
  const cfg = (await bob.get('/crm/config')).data.types.find((t: any) => t.code === 'deal');
  ok('…nor the field', !cfg.custom_fields.some((f: any) => f.code === m) && cfg.custom_fields.some((f: any) => f.code === g));
  ok('…nor in the field list', !(await bob.get('/crm/deal/fields')).data.some((f: any) => f.code === m));
  ok('…nor on the card layout', !JSON.stringify((await bob.get('/crm/deal/card-layout')).data).includes(m));
  const list = (await bob.get(`/crm/deal/items?q=${encodeURIComponent(`Deal ${stamp}`)}`)).data.items;
  ok('…nor in the list', list.length > 0 && list.every((i: any) => !(m in i.uf)), list.map((i: any) => i.uf));
  const board = (await bob.get('/crm/deal/kanban')).data.columns.flatMap((c: any) => c.items);
  ok('…nor on the board', board.some((i: any) => i.id === deal) && board.every((i: any) => !(m in i.uf)));
  ok('the chosen see it in the list', (await ann.get(`/crm/deal/items?q=${encodeURIComponent(`Deal ${stamp}`)}`)).data.items.some((i: any) => i.uf[m] === 42));

  r = await bob.get(`/crm/deal/items?uf=${encodeURIComponent(JSON.stringify({ [m]: 42 }))}`);
  ok('the others can\'t filter by it (it would reveal values)', r.status === 400, r.data);
  ok('…the chosen can', (await ann.get(`/crm/deal/items?uf=${encodeURIComponent(JSON.stringify({ [m]: 42 }))}`)).data.items.some((i: any) => i.id === deal));
  r = await bob.patch(`/crm/deal/items/${deal}`, { uf: { [m]: 1 } });
  ok('…nor write it', r.status === 400, r.data);
  ok('…and the value stays', (await ann.get(`/crm/deal/items/${deal}`)).data.uf[m] === 42);
  r = await bob.patch(`/crm/deal/items/${deal}`, { uf: { [g]: 'South' } });
  ok('their own edits keep the hidden value', r.status === 200 && (await ann.get(`/crm/deal/items/${deal}`)).data.uf[m] === 42, r.data);

  const csv = await (await bob.req('GET', `/crm/deal/export?q=${encodeURIComponent(`Deal ${stamp}`)}`, undefined, true)).text();
  ok('exports leave it out for the others', csv.includes(`Region ${stamp}`) && !csv.includes(`Margin ${stamp}`), csv.slice(0, 300));
  const csvA = await (await ann.req('GET', `/crm/deal/export?q=${encodeURIComponent(`Deal ${stamp}`)}`, undefined, true)).text();
  ok('…and keep it for the chosen', csvA.includes(`Margin ${stamp}`) && csvA.includes('42'));
  r = await bob.req('GET', `/crm/deal/export?uf=${encodeURIComponent(JSON.stringify({ [m]: 42 }))}`, undefined, true) as any;
  ok('…nor filter exports by it', r.status === 400);

  const tl = async (c: Client) => JSON.stringify((await c.get(`/crm/items/${deal}/timeline`)).data.items.filter((e: any) => e.kind === 'edit').map((e: any) => e.data.changes));
  ok('the history hides its changes from the others', !(await tl(bob)).includes(m) && (await tl(bob)).includes(`uf.${g}`), await tl(bob));
  ok('…and shows them to the chosen', (await tl(ann)).includes(`uf.${m}`));

  const tpl = (await bob.post('/crm/email-templates', { name: `T ${stamp}`, subject: 'x', body: `M={{uf.${m}}} R={{uf.${g}}}` })).data;
  r = await bob.post(`/crm/email-templates/${tpl.id}/render`, { item_id: deal });
  ok('e-mail templates leave it blank for the others', r.status === 200 && r.data.body.includes('R=South') && !r.data.body.includes('42'), r.data);

  // Changing who sees it.
  await admin.patch(`/crm/deal/fields/${margin.id}`, { visible_user_ids: [], visible_department_ids: [] });
  ok('nobody chosen: everyone sees it', (await bob.get(`/crm/deal/items/${deal}`)).data.uf[m] === 42);
  await admin.patch(`/crm/deal/fields/${margin.id}`, { visible_user_ids: [bob.id] });
  ok('…then only Bob', (await bob.get(`/crm/deal/items/${deal}`)).data.uf[m] === 42 && !(m in (await ann.get(`/crm/deal/items/${deal}`)).data.uf));
  ok('only CRM admins choose', (await ann.patch(`/crm/deal/fields/${margin.id}`, { visible_user_ids: [] })).status === 403);
} finally {
  for (const id of fields) await admin.del(`/crm/deal/fields/${id}`);
  await admin.del(`/roles/${dealRole.id}`);
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
