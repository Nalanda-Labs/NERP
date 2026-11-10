// Edit history, observers (access + notifications), tasks linked to CRM records, email
// templates.
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
  const r = await admin.post('/users', { email, password: 'password123', first_name: name, last_name: 'Collab' });
  const c = new Client(); await c.post('/auth/login', { email, password: 'password123' }); c.id = r.data.id; return c;
}
const sam = await mk('sam'), ivy = await mk('ivy');
// Employees read only their own deals and companies here, so access by observing shows.
const empRole = (await admin.get('/roles')).data.find((x) => x.code === 'employee');
const perms = (await admin.get('/crm/permissions')).data;
const original = Object.entries(perms.matrix[empRole.id]).flatMap(([t, acts]: [string, any]) => Object.entries(acts).map(([action, scope]) => ({ entity_type: +t, action, scope })));
await admin.put(`/crm/permissions/${empRole.id}`, { entries: original.map((e) => ([2, 4].includes(e.entity_type) && e.action === 'read' ? { ...e, scope: 'own' } : e)) });
try {
const waitFor = async (fn, ms = 15000) => { const end = Date.now() + ms; let v; while (Date.now() < end) { v = await fn(); if (v) return v; await sleep(300); } return v; };
const timeline = async (c, id) => (await c.get(`/crm/items/${id}/timeline`)).data.items ?? [];
const notes = async (c) => (await c.get('/notifications?limit=50')).data.items ?? [];
const made: any[] = [];

// ---- edit history ----
const cfg = (await admin.get('/crm/config')).data;
const fd = (await admin.post('/crm/deal/fields', { label: `Tier ${stamp}`, kind: 'string' })).data;
const company = (await sam.post('/crm/company/items', { company_name: `Hist Co ${stamp}` })).data;
const deal = (await sam.post('/crm/deal/items', { title: `History ${stamp}`, opportunity: '100', currency: cfg.base_currency, close_date: '2027-01-10', assigned_to: sam.id })).data; made.push(['deal', deal.id]);
let r: any = await sam.patch(`/crm/deal/items/${deal.id}`, { title: `History ${stamp} v2`, opportunity: '250', close_date: '2027-02-01', company_id: company.id, uf: { [fd.code]: 'Gold' } });
ok('save works', r.status === 200, r.data);
let tl = await timeline(sam, deal.id);
const edit = tl.find((e) => e.kind === 'edit');
const ch = Object.fromEntries((edit?.data.changes ?? []).map((c) => [c.field, c]));
ok('one history entry with every change', edit && ch.title?.from === `History ${stamp}` && ch.title.to === `History ${stamp} v2` && ch.close_date?.from === '2027-01-10' && ch.close_date.to === '2027-02-01', edit);
ok('amount, company and custom field changes', +ch.opportunity?.from === 100 && +ch.opportunity.to === 250 && ch.company_id?.to === `Hist Co ${stamp}` && ch[`uf.${fd.code}`]?.to === 'Gold', ch);
ok('who changed it', edit?.author_id === sam.id);
await sam.patch(`/crm/deal/items/${deal.id}`, { title: `History ${stamp} v2` });
ok('no entry when nothing changed', (await timeline(sam, deal.id)).filter((e) => e.kind === 'edit').length === 1);
const contact = (await sam.post('/crm/contact/items', { first_name: 'Hal', last_name: `Story${stamp}` })).data; made.push(['contact', contact.id]);
await sam.patch(`/crm/contact/items/${contact.id}`, { last_name: `Storey${stamp}`, multifields: [{ kind: 'phone', value_type: 'work', value: '+1 555 0100' }] });
const ce = (await timeline(sam, contact.id)).find((e) => e.kind === 'edit');
const cf = (ce?.data.changes ?? []).map((c) => c.field);
ok('contact: name and phone changes, title not repeated', cf.includes('last_name') && cf.includes('multifields') && !cf.includes('title'), ce);
await sam.patch(`/crm/deal/items/${deal.id}`, { stage_id: cfg.types.find((t) => t.code === 'deal').categories.find((c) => c.is_default).stages[1].id });
ok('stage moves are not doubled as edits', (await timeline(sam, deal.id)).filter((e) => e.kind === 'edit').length === 1);

// ---- observers ----
ok('ivy cannot open sam\'s deal yet', (await ivy.get(`/crm/deal/items/${deal.id}`)).status === 404);
ok('observers must be active people', (await sam.put(`/crm/items/${deal.id}/observers`, { user_ids: ['00000000-0000-0000-0000-000000000000'] })).status === 400);
ok('ivy cannot add herself', (await ivy.put(`/crm/items/${deal.id}/observers`, { user_ids: [ivy.id] })).status === 404);
r = await sam.put(`/crm/items/${deal.id}/observers`, { user_ids: [ivy.id] });
ok('sam adds ivy as an observer', r.status === 200 && r.data.observers[0]?.id === ivy.id, r.data);
ok('ivy is told', !!(await waitFor(async () => (await notes(ivy)).find((n) => n.kind === 'crm.observed' && n.title.includes('follow')))));
r = await ivy.get(`/crm/deal/items/${deal.id}`);
ok('observer can open the record', r.status === 200 && r.data.observers.some((o) => o.id === ivy.id) && r.data.can_write === false, r.status);
ok('observer sees it in lists', (await ivy.get(`/crm/deal/items?q=${encodeURIComponent('History ' + stamp)}`)).data.items?.some((d) => d.id === deal.id));
ok('observer sees the timeline and products', (await ivy.get(`/crm/items/${deal.id}/timeline`)).status === 200 && (await ivy.get(`/crm/items/${deal.id}/products`)).status === 200);
ok('observer cannot edit', [403, 404].includes((await ivy.patch(`/crm/deal/items/${deal.id}`, { title: 'x' })).status));
r = await ivy.post(`/crm/items/${deal.id}/timeline`, { body: 'Following this one' });
ok('observer can comment', r.status === 200, r.data);
ok('responsible is told about the comment', !!(await waitFor(async () => (await notes(sam)).find((n) => n.kind === 'crm.observed' && n.title.includes('New comment')))));
await sam.patch(`/crm/deal/items/${deal.id}`, { stage_id: cfg.types.find((t) => t.code === 'deal').categories.find((c) => c.is_default).stages[2].id });
ok('observer told about stage moves', !!(await waitFor(async () => (await notes(ivy)).find((n) => n.kind === 'crm.observed' && n.title.includes('moved to')))));
ok('timeline notes observers', (await timeline(sam, deal.id)).some((e) => e.kind === 'observers' && e.data.added?.length === 1));
await sam.put(`/crm/items/${deal.id}/observers`, { user_ids: [] });
ok('removed observer loses access', (await ivy.get(`/crm/deal/items/${deal.id}`)).status === 404);

// ---- robot adds observers ----
const cat = cfg.types.find((t) => t.code === 'deal').categories.find((c) => c.is_default);
r = await admin.post('/crm/robots', { category_id: cat.id, stage_id: cat.stages[3].id, name: `Watch ${stamp}`, action: 'observers', params: { users: [ivy.id] }, conditions: [{ field: 'title', op: 'contains', value: stamp }] });
ok('observers robot saved', r.status === 200, r.data);
const robotId = r.data.id;
await sam.patch(`/crm/deal/items/${deal.id}`, { stage_id: cat.stages[3].id });
ok('robot adds the observer', !!(await waitFor(async () => (await ivy.get(`/crm/deal/items/${deal.id}`)).status === 200)));
await admin.del(`/crm/robots/${robotId}`);

// ---- tasks linked to records ----
r = await sam.post('/tasks', { title: `Call about ${stamp}`, crm_item_ids: [deal.id] });
const task = r.data;
ok('task linked to the deal', r.status === 200 && task.crm?.[0]?.id === deal.id && task.crm[0].code === 'deal', r.data);
r = await sam.get(`/crm/items/${deal.id}/tasks`);
ok('deal lists its tasks', r.data.items?.some((t) => t.id === task.id), r.data);
ok('timeline notes the task', (await timeline(sam, deal.id)).some((e) => e.kind === 'task' && e.data.event === 'created' && e.data.task_id === task.id));
ok('cannot link a record you cannot see', (await ivy.post('/tasks', { title: 'x', crm_item_ids: [company.id] })).status === 404);
await sam.post(`/tasks/${task.id}/status`, { status: 'done' });
ok('completing it is noted', !!(await waitFor(async () => (await timeline(sam, deal.id)).some((e) => e.kind === 'task' && e.data.event === 'completed'))));
r = await sam.put(`/tasks/${task.id}/crm`, { item_ids: [deal.id, contact.id] });
ok('links can change', r.status === 200 && r.data.crm.length === 2, r.data?.crm);
r = await sam.put(`/tasks/${task.id}/crm`, { item_ids: [contact.id] });
ok('and be removed', r.data.crm.length === 1 && r.data.crm[0].id === contact.id);
ok('ivy cannot change links of sam\'s task', [403, 404].includes((await ivy.put(`/tasks/${task.id}/crm`, { item_ids: [] })).status));
const auto = (await admin.post('/crm/robots', { category_id: cat.id, stage_id: cat.stages[4]?.id ?? cat.stages[3].id, name: `Task ${stamp}`, action: 'task', params: { title: 'Robot task {{title}}', responsible: 'assigned' }, conditions: [{ field: 'title', op: 'contains', value: stamp }] })).data;
await sam.patch(`/crm/deal/items/${deal.id}`, { stage_id: auto.stage_id });
ok('robot tasks are linked', !!(await waitFor(async () => (await sam.get(`/crm/items/${deal.id}/tasks`)).data.items?.some((t) => t.title.startsWith('Robot task')))));
await admin.del(`/crm/robots/${auto.id}`);

// ---- email templates ----
r = await sam.post('/crm/email-templates', { name: `Follow-up ${stamp}`, subject: 'About {{title}}', body: 'Hello {{contact}},\n{{me.name}}', entity_types: [2] });
const mine = r.data;
ok('personal template', r.status === 200 && mine.owner_id === sam.id, r.data);
ok('employees cannot share', (await sam.post('/crm/email-templates', { name: 'x', shared: true })).status === 403);
r = await admin.post('/crm/email-templates', { name: `Shared ${stamp}`, subject: 'Hi', body: 'Body', shared: true });
const shared = r.data;
ok('admin shares one', r.status === 200 && shared.owner_id === null);
r = await sam.get('/crm/email-templates?entity_type=2');
ok('sam sees shared and own', r.data.items.some((t) => t.id === mine.id) && r.data.items.some((t) => t.id === shared.id) && r.data.can_share === false);
ok('filtered by record type', !(await sam.get('/crm/email-templates?entity_type=3')).data.items.some((t) => t.id === mine.id));
ok('ivy does not see sam\'s', !(await ivy.get('/crm/email-templates')).data.items.some((t) => t.id === mine.id));
ok('ivy cannot change it', (await ivy.patch(`/crm/email-templates/${mine.id}`, { name: 'x' })).status === 404);
ok('sam cannot change the shared one', (await sam.patch(`/crm/email-templates/${shared.id}`, { name: 'x' })).status === 403);
r = await sam.post(`/crm/email-templates/${mine.id}/render`, { item_id: deal.id });
ok('rendered for a record', r.status === 200 && r.data.subject === `About History ${stamp} v2` && r.data.body.includes('sam Collab'), r.data);
ok('render needs access to the record', (await ivy.post(`/crm/email-templates/${shared.id}/render`, { item_id: company.id })).status === 404);
ok('template name required', (await sam.post('/crm/email-templates', { name: ' ' })).status === 400);
await sam.del(`/crm/email-templates/${mine.id}`); await admin.del(`/crm/email-templates/${shared.id}`);

// ---- cleanup ----
await admin.del(`/tasks/${task.id}`);
for (const t of (await admin.get(`/crm/items/${deal.id}/tasks`)).data.items ?? []) await admin.del(`/tasks/${t.id}`);
made.push(['company', company.id]);
for (const [code, id] of made) { await admin.del(`/crm/${code}/items/${id}`); await admin.del(`/crm/recycle-bin/${id}`); }
await admin.del(`/crm/deal/fields/${fd.id}`);
} finally {
  await admin.put(`/crm/permissions/${empRole.id}`, { entries: original });
}
console.log(`\n${pass} passed, ${fail} failed`);
await cleanupRun(stamp);
process.exit(fail ? 1 : 0);
