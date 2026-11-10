import { execSync } from 'node:child_process';
import { cleanupRun } from './cleanup.ts';
import { Client } from './lib.ts';
const B = 'http://localhost:8080/api';
let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 500)); };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const stamp = Date.now().toString(36);
const admin = new Client(); admin.id = (await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' })).data.user.id;
async function mk(name) {
  const email = `${name}.${stamp}@acme.test`;
  const r = await admin.post('/users', { email, password: 'password123', first_name: name, last_name: 'Crm' });
  const c = new Client(); await c.post('/auth/login', { email, password: 'password123' }); c.id = r.data.id; return c;
}
const sam = await mk('sam'), ivy = await mk('ivy');

async function dropTestFields() {
  for (const t of ['deal', 'lead']) for (const f of (await admin.get(`/crm/${t}/fields`)).data) if (['Region', 'Contract value', 'Campaign'].includes(f.label)) await admin.del(`/crm/${t}/fields/${f.id}`);
}
await dropTestFields();


// ---- config & currencies ----
let r: any = await admin.get('/crm/config');
ok('config', r.status === 200 && r.data.types.filter((t) => t.is_system).length === 4 && r.data.types.some((t) => t.kind === "invoice") && r.data.base_currency, r.data);
const deal = r.data.types.find((t) => t.code === 'deal');
const sales = deal.categories.find((c) => c.is_default);
const st = Object.fromEntries(sales.stages.map((s) => [s.name, s]));
ok('deal pipeline seeded', ['New', 'Won', 'Lost'].every((n) => st[n]) && st.Won.semantics === 'success');
ok('employee rights default', (await sam.get('/crm/config')).data.types[1].rights.write === 'own');
await admin.post('/crm/currencies', { code: 'EUR', rate: '1.10' });
await admin.post('/crm/currencies', { code: 'inr', rate: '0.012' });
r = await admin.get('/crm/currencies');
ok('currencies enabled', ['USD', 'EUR', 'INR'].every((c) => r.data.some((x) => x.code === c)), r.data);
ok('bad currency code rejected', (await admin.post('/crm/currencies', { code: 'EURO', rate: '1' })).status === 400);
ok('non-admin cannot add currency', (await sam.post('/crm/currencies', { code: 'GBP', rate: '1.3' })).status === 403);

// ---- records ----
r = await sam.post('/crm/company/items', { company_name: `Globex ${stamp}`, industry: 'manufacturing', company_kind: 'customer',
  address: { line1: '1 Main St', city: 'Springfield', country: 'US' }, multifields: [{ kind: 'web', value: 'https://globex.example' }] });
ok('create company', r.status === 200 && r.data.title === `Globex ${stamp}` && r.data.address.country === 'US', r.data);
const company = r.data.id;
ok('bad country rejected', (await sam.post('/crm/company/items', { company_name: 'x', address: { country: 'USA' } })).status === 400);
r = await sam.post('/crm/contact/items', { first_name: 'Hank', last_name: `Scorpio${stamp}`, job_title: 'CEO', company_id: company,
  multifields: [{ kind: 'phone', value_type: 'mobile', value: '+1 (555) 010-7788' }, { kind: 'email', value: `Hank.${stamp}@Globex.example` }] });
ok('create contact', r.status === 200 && r.data.title === `Hank Scorpio${stamp}` && r.data.multifields.length === 2 && r.data.company_title === `Globex ${stamp}`, r.data);
const contact = r.data.id;
ok('invalid email rejected', (await sam.post('/crm/contact/items', { first_name: 'x', multifields: [{ kind: 'email', value: 'nope' }] })).status === 400);
ok('unknown field rejected', (await sam.post('/crm/contact/items', { first_name: 'x', opportunity: 5 })).status === 400);
r = await sam.post('/crm/deal/items', { title: `Big order ${stamp}`, opportunity: '1000', currency: 'EUR', company_id: company, contact_ids: [contact], close_date: '2026-12-31', probability: 60 });
ok('create deal', r.status === 200 && r.data.stage_id === st.New.id && +r.data.opportunity === 1000 && +r.data.opportunity_base === 1100 && r.data.contacts[0]?.id === contact, r.data);
const d1 = r.data.id;
ok('number assigned', r.data.number > 0);
ok('company shows related deal + contact', (await sam.get(`/crm/company/items/${company}`)).data.related.length === 2);

// ---- search, duplicates, filters ----
ok('search by name (trigram)', (await sam.get(`/crm/contact/items?q=scorpio${stamp}`)).data.items[0]?.id === contact);
ok('search by phone digits', (await sam.get('/crm/contact/items?q=5550107788')).data.items.some((x) => x.id === contact));
ok('search by email', (await sam.get(`/crm/contact/items?q=hank.${stamp}`)).data.items.some((x) => x.id === contact));
ok('short search rejected', (await sam.get('/crm/contact/items?q=ab')).status === 400);
r = await sam.get(`/crm/contact/duplicates?phones=${encodeURIComponent('555-010-7788')}&emails=${encodeURIComponent('HANK.' + stamp + '@globex.example')}`);
ok('duplicate detection (phone+email, any format)', r.data.items.some((x) => x.id === contact), r.data);
ok('list row has phone/email', (await sam.get(`/crm/contact/items?q=scorpio${stamp}`)).data.items[0].phone === '+1 (555) 010-7788');
ok('filter amount (base currency)', (await sam.get('/crm/deal/items?amount_min=1050&assigned_to=me')).data.items.some((x) => x.id === d1));
ok('filter amount excludes', !(await sam.get('/crm/deal/items?amount_min=1200&assigned_to=me')).data.items.some((x) => x.id === d1));
ok('filter by company', (await sam.get(`/crm/deal/items?company_id=${company}`)).data.total === 1);

// ---- custom fields ----
r = await admin.post('/crm/deal/fields', { label: 'Region', kind: 'list', settings: { options: [{ id: 'north', label: 'North' }, { id: 'south', label: 'South' }] } });
ok('create list field', r.status === 200 && r.data.code === 'uf_region', r.data);
await admin.post('/crm/deal/fields', { label: 'Contract value', kind: 'money' });
r = await sam.patch(`/crm/deal/items/${d1}`, { uf: { uf_region: 'north', uf_contract_value: { amount: '99.5', currency: 'EUR' } } });
ok('set custom fields', r.data.uf.uf_region === 'north' && r.data.uf.uf_contract_value.amount === '99.5', r.data.uf);
ok('invalid option rejected', (await sam.patch(`/crm/deal/items/${d1}`, { uf: { uf_region: 'west' } })).status === 400);
ok('filter by custom field', (await sam.get(`/crm/deal/items?uf=${encodeURIComponent(JSON.stringify({ uf_region: 'north' }))}`)).data.items.some((x) => x.id === d1));
await admin.post('/crm/lead/fields', { label: 'Campaign', kind: 'string', is_required: true });
ok('required custom field enforced', (await sam.post('/crm/lead/items', { title: 'x' })).status === 400);

// ---- products & tax ----
r = await admin.post('/crm/products', { name: `Widget ${stamp}`, price: '100', currency: 'EUR', tax_rate: '19', sku: 'W-1' });
ok('create product', r.status === 200 && +r.data.price === 100, r.data);
const prod = r.data.id;
r = await sam.put(`/crm/items/${d1}/products`, { rows: [
  { product_id: prod, name: 'Widget', price: '100', quantity: '3', discount_kind: 'percent', discount_value: '10', tax_rate: '19' },
  { name: 'Setup', price: '50', quantity: '1', tax_rate: '19', tax_included: true } ] });
ok('product rows totals', r.status === 200 && r.data.totals.total === '371.30' && r.data.totals.tax === '59.28', r.data.totals);
r = await sam.get(`/crm/deal/items/${d1}`);
ok('deal amount follows products', +r.data.opportunity === 371.3 && !r.data.is_manual_amount && +r.data.opportunity_base === 408.43, [r.data.opportunity, r.data.opportunity_base]);

// ---- kanban & stages ----
r = await sam.get('/crm/deal/kanban');
ok('kanban columns', r.status === 200 && r.data.columns.length === sales.stages.length && r.data.columns[0].items.some((x) => x.id === d1), r.data.columns?.map((c) => c.count));
r = await sam.post(`/crm/deal/items/${d1}/move`, { stage_id: st.Won.id });
ok('move to Won closes', r.data.is_closed && r.data.stage_semantics === 'success' && !!r.data.closed_at);
r = await sam.get(`/crm/items/${d1}/timeline`);
ok('timeline: created + stage + products', ['created', 'stage', 'products'].every((k) => r.data.items.some((e) => e.kind === k)), r.data.items.map((e) => e.kind));
ok('stage of other pipeline rejected', (await sam.patch(`/crm/deal/items/${d1}`, { stage_id: (await admin.get('/crm/config')).data.types.find((t) => t.code === 'lead').categories[0].stages[0].id })).status === 400);
r = await admin.post('/crm/deal/categories', { name: `Partners ${stamp}` });
ok('new pipeline copies stages', r.data.categories.some((c) => c.name === `Partners ${stamp}` && c.stages.length === sales.stages.length));
const partners = r.data.categories.find((c) => c.name === `Partners ${stamp}`);
r = await sam.patch(`/crm/deal/items/${d1}`, { category_id: partners.id });
ok('move to other pipeline lands in first open stage', r.data.category_id === partners.id && r.data.stage_id === partners.stages[0].id && !r.data.is_closed);

// ---- activities & counters ----
r = await sam.post('/crm/activities', { item_id: d1, kind: 'call', subject: 'Follow-up call', deadline: new Date(Date.now() - 3600e3).toISOString() });
ok('create activity', r.status === 200 && r.data.is_overdue);
const act = r.data.id;
r = await sam.get('/crm/counters');
ok('counters show overdue', r.data.activities_overdue >= 1 && r.data.types['2'].overdue >= 1, r.data);
ok('activity filter on list', (await sam.get('/crm/deal/items?activity=overdue')).data.items.some((x) => x.id === d1));
r = await sam.post(`/crm/activities/${act}/complete`, { result: 'Agreed on terms' });
ok('complete activity', r.data.is_completed);
ok('logged call without deadline is done', (await sam.post('/crm/activities', { item_id: contact, kind: 'call', subject: 'Intro call', direction: 'incoming' })).data.is_completed);
r = await sam.post('/crm/activities', { item_id: d1, kind: 'meeting', subject: 'Demo', starts_at: new Date(Date.now() + 5 * 60e3).toISOString(), remind_before_minutes: 15, responsible_id: ivy.id });
ok('assign activity to colleague', r.data.responsible_id === ivy.id);

// ---- timeline comments ----
r = await sam.post(`/crm/items/${d1}/timeline`, { body: 'Customer asked for a discount' });
const tl = (await sam.get(`/crm/items/${d1}/timeline`)).data.items.find((e) => e.kind === 'comment');
ok('comment added', !!tl);
await sam.patch(`/crm/timeline/${tl.id}`, { is_pinned: true });
ok('pinned', (await sam.get(`/crm/items/${d1}/timeline`)).data.pinned.length === 1);

// ---- access scopes ----
ok('colleague can read (all)', (await ivy.get(`/crm/deal/items/${d1}`)).status === 200);
r = await ivy.get(`/crm/deal/items/${d1}`);
ok('colleague sees can_write=false', r.data.can_write === false);
ok('colleague cannot edit (own only)', (await ivy.patch(`/crm/deal/items/${d1}`, { title: 'x' })).status === 403);
const empRole = (await admin.get('/roles')).data.find((x) => x.code === 'employee');
const perms = (await admin.get('/crm/permissions')).data;
const original = Object.entries(perms.matrix[empRole.id]).flatMap(([t, acts]: [string, any]) => Object.entries(acts).map(([action, scope]) => ({ entity_type: +t, action, scope })));
await admin.put(`/crm/permissions/${empRole.id}`, { entries: original.map((e) => (e.entity_type === 2 && e.action === 'read' ? { ...e, scope: 'own' } : e)) });
ok('read=own hides others\' deals', (await ivy.get(`/crm/deal/items/${d1}`)).status === 404 && !(await ivy.get('/crm/deal/items')).data.items.some((x) => x.id === d1));
await admin.put(`/crm/permissions/${empRole.id}`, { entries: original });
ok('permissions restored', (await ivy.get(`/crm/deal/items/${d1}`)).status === 200);

// ---- lead conversion ----
r = await sam.post('/crm/lead/items', { title: `Inbound ${stamp}`, first_name: 'Marge', last_name: `Lead${stamp}`, company_name: `Kwik ${stamp}`,
  opportunity: '500', currency: 'USD', source: 'website', multifields: [{ kind: 'email', value: `marge.${stamp}@kwik.example` }, { kind: 'web', value: 'kwik.example' }], uf: { uf_campaign: 'Spring' } });
ok('create lead', r.status === 200 && r.data.stage_semantics === 'process', r.data);
const lead = r.data.id;
r = await sam.post(`/crm/lead/items/${lead}/convert`, { create_contact: true, create_company: true, create_deal: true });
ok('convert lead', r.status === 200 && r.data.contact_id && r.data.company_id && r.data.deal_id, r.data);
const conv = r.data;
r = await sam.get(`/crm/deal/items/${conv.deal_id}`);
ok('deal from lead keeps amount + links', +r.data.opportunity === 500 && r.data.company_id === conv.company_id && r.data.contacts[0]?.id === conv.contact_id && r.data.source_item?.id === lead, r.data);
ok('contact from lead has email', (await sam.get(`/crm/contact/items/${conv.contact_id}`)).data.multifields.some((m) => m.kind === 'email'));
ok('lead closed as converted', (await sam.get(`/crm/lead/items/${lead}`)).data.stage_semantics === 'success');
ok('second conversion refused', (await sam.post(`/crm/lead/items/${lead}/convert`, { create_deal: true })).status === 400);

// ---- merge ----
const c2 = (await sam.post('/crm/contact/items', { first_name: 'Hank', last_name: `Scorpio${stamp}`, birthdate: '1970-01-01', multifields: [{ kind: 'phone', value: '+1 555 999 0000' }] })).data.id;
r = await sam.post('/crm/contact/merge', { target_id: contact, source_ids: [c2] });
ok('merge contacts', r.status === 200);
r = await sam.get(`/crm/contact/items/${contact}`);
ok('merged: gaps filled, phones combined', r.data.birthdate === '1970-01-01' && r.data.multifields.filter((m) => m.kind === 'phone').length === 2, r.data);
ok('merged source gone', (await sam.get(`/crm/contact/items/${c2}`)).status === 404);

// ---- recycle bin ----
const tmp = (await sam.post('/crm/company/items', { company_name: `Temp ${stamp}` })).data.id;
await sam.del(`/crm/company/items/${tmp}`);
ok('deleted hidden from list', !(await sam.get(`/crm/company/items?q=temp ${stamp}`)).data.items.length);
ok('in recycle bin', (await sam.get('/crm/recycle-bin?type=company')).data.page.items.some((x) => x.id === tmp));
await sam.post(`/crm/recycle-bin/${tmp}/restore`);
ok('restored', (await sam.get(`/crm/company/items/${tmp}`)).status === 200);
await sam.del(`/crm/company/items/${tmp}`); await sam.del(`/crm/recycle-bin/${tmp}`);
ok('purged', (await sam.get('/crm/recycle-bin?type=company')).data.page.items.every((x) => x.id !== tmp));

// ---- bulk ----
const b1 = (await sam.post('/crm/deal/items', { title: `Bulk A ${stamp}` })).data.id, b2 = (await sam.post('/crm/deal/items', { title: `Bulk B ${stamp}` })).data.id;
r = await sam.post('/crm/deal/bulk', { ids: [b1, b2, d1], action: 'stage', value: st.Lost.id });
ok('bulk stage (a stage from another pipeline moves the deal there)', r.data.done === 3 && (await sam.get(`/crm/deal/items/${d1}`)).data.category_id === sales.id, r.data);
r = await sam.post('/crm/deal/bulk', { ids: [b1, b2], action: 'assign', value: ivy.id });
ok('bulk assign', r.data.done === 2 && (await ivy.get(`/crm/deal/items/${b1}`)).data.assigned_to === ivy.id);
ok('assignee notified', (await ivy.get('/notifications?limit=20')).data.items.some((n) => n.kind === 'crm.assigned'));

// ---- export ----
const exp = await sam.req('GET', `/crm/contact/export?q=scorpio${stamp}`, undefined, true);
const csvText = await exp.text();
ok('export CSV', exp.status === 200 && exp.headers.get('content-type')!.startsWith('text/csv') && csvText.split('\r\n')[0].startsWith('Number,Title') && csvText.includes(`Scorpio${stamp}`), csvText.slice(0, 200));

// ---- import ----
await admin.put(`/crm/permissions/${empRole.id}`, { entries: original.map((e) => (e.action === 'import' && e.entity_type === 3 ? { ...e, scope: 'all' } : e)) });
const csv = `First,Last,Phone,Email,Company,Country\nAna,Imp${stamp},+44 20 ${String(Date.now()).slice(-8)},ana.${stamp}@ex.example,Imported Co ${stamp},GB\nBob,Imp${stamp},,bob.${stamp}@ex.example,Imported Co ${stamp},FR\nHank,Dup,555 010 7788,,,\n`;
const fd = new FormData(); fd.append('file', new Blob([csv], { type: 'text/csv' }), `contacts-${stamp}.csv`);
const file = (await sam.post('/files', fd)).data;
r = await sam.post('/crm/contact/import', { file_id: file.id, mapping: { 0: 'first_name', 1: 'last_name', 2: 'phone', 3: 'email', 4: 'company', 5: 'address.country' }, duplicates: 'skip' });
ok('import queued', r.status === 200 && r.data.job_id, r.data);
let st2: any = null;
for (let i = 0; i < 40; i++) { await sleep(500); st2 = (await sam.get(`/crm/imports/${r.data.job_id}`)).data; if (st2.state === 'done') break; }
ok('import finished: 2 created, 1 duplicate skipped', st2?.state === 'done' && st2.created === 2 && st2.skipped === 1, st2);
r = await sam.get(`/crm/contact/items?q=imp${stamp}`);
ok('imported contacts linked to one new company', r.data.total === 2 && r.data.items.every((x) => x.company_title === `Imported Co ${stamp}`) && (await sam.get(`/crm/company/items?q=imported co ${stamp}`)).data.total === 1, r.data.items.map((x) => x.company_title));
await admin.put(`/crm/permissions/${empRole.id}`, { entries: original });

// ---- reminders ----
execSync('redis-cli del jobs:recurring:crm.reminders');
let reminded = false;
for (let i = 0; i < 75 && !reminded; i++) { await sleep(1000); if (i % 15 === 14) execSync('redis-cli del jobs:recurring:crm.reminders'); reminded = (await ivy.get('/notifications?limit=30')).data.items.some((n) => n.kind === 'crm.reminder'); }
ok('activity reminder sent', reminded);

await dropTestFields();
// Remove the test pipeline (its records must leave the recycle bin first).
await admin.del(`/crm/deal/items/${d1}`); await admin.del(`/crm/recycle-bin/${d1}`);
r = await admin.del(`/crm/categories/${partners.id}`);
ok('cleanup: test pipeline removed', r.status === 200, r.data);
console.log(`\n${pass} passed, ${fail} failed`);
await cleanupRun(stamp);
