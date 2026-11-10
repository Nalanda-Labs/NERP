// Smart processes: admin-defined record types on the CRM item engine.
import { execSync } from 'node:child_process';
import { writeFileSync } from 'node:fs';
import { cleanupRun } from './cleanup.ts';
import { Client } from './lib.ts';
const B = 'http://localhost:8080/api';
let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 600)); };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const stamp = Date.now().toString(36);
const admin = new Client(); admin.id = (await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' })).data.user.id;
async function mk(name) {
  const email = `${name}.${stamp}@acme.test`;
  const r = await admin.post('/users', { email, password: 'password123', first_name: name, last_name: 'Doc' });
  const c = new Client(); await c.post('/auth/login', { email, password: 'password123' }); c.id = r.data.id; return c;
}
const sam = await mk('sam'), ivy = await mk('ivy');
const waitFor = async (fn, ms = 20000) => { const end = Date.now() + ms; let v; while (Date.now() < end) { v = await fn(); if (v) return v; await sleep(300); } return v; };

// ---- create a type ----
ok('non-admin cannot create types', (await sam.post('/crm/types', { name: 'X' })).status === 403);
let r: any = await admin.post('/crm/types', { name: `Contract ${stamp}`, name_plural: `Contracts ${stamp}`, icon: 'file', has_amount: true, has_client: true });
ok('create smart process', r.status === 200 && r.data.code === `contracts-${stamp}` && r.data.id >= 128 && !r.data.is_system && r.data.has_stages, r.data);
const T = r.data, code = T.code;
ok('bad icon rejected', (await admin.post('/crm/types', { name: 'Y', icon: 'rocket' })).status === 400);
r = await admin.post('/crm/types', { name: 'Deal', name_plural: 'Settings' });
ok('reserved code avoided', r.status === 200 && r.data.code === `type-${r.data.id}`, r.data);
const reservedType = r.data.code;
r = await sam.get('/crm/config');
const cfg = r.data.types.find((t) => t.code === code);
ok('in config with stages, fields and rights', cfg && cfg.categories.length === 1 && cfg.categories[0].stages.map((s) => s.name).join() === 'New,In progress,Done,Cancelled' && cfg.system_fields.some((f) => f.code === 'opportunity') && !cfg.system_fields.some((f) => f.code === 'probability') && cfg.rights.read === 'all' && cfg.rights.write === 'own', cfg);
ok('built-ins have flags', r.data.types.find((t) => t.code === 'deal').has_client && r.data.types.find((t) => t.code === 'contact').is_system);

// ---- records ----
const co = (await sam.post('/crm/company/items', { company_name: `Client ${stamp}` })).data.id;
r = await sam.post(`/crm/${code}/items`, { title: `Master agreement ${stamp}`, opportunity: '12000', currency: 'EUR', company_id: co, close_date: '2026-12-31', source: 'website' });
ok('create record', r.status === 200 && r.data.title === `Master agreement ${stamp}` && r.data.stage_name === 'New' && r.data.company_id === co && r.data.number === 1, r.data);
const item = r.data.id;
ok('deal-only field rejected', (await sam.post(`/crm/${code}/items`, { title: 'x', probability: 50 })).status === 400);
ok('person field rejected', (await sam.post(`/crm/${code}/items`, { first_name: 'x' })).status === 400);
r = await sam.get(`/crm/${code}/kanban`);
ok('kanban works', r.status === 200 && JSON.stringify(r.data).includes(item), r.data);
r = await sam.get(`/crm/${code}/items?q=${encodeURIComponent(`Master agreement ${stamp}`)}`);
ok('list + search', r.data.items?.length === 1);
r = await sam.put(`/crm/items/${item}/products`, { rows: [{ product_id: null, name: 'Service', price: '1000', quantity: '12', unit: 'mo', discount_kind: 'percent', discount_value: '0', tax_rate: '19', tax_included: false }] });
ok('products on a smart process', r.status === 200 && r.data.totals.total === '14280.00', r.data);
const stages = cfg.categories[0].stages;
r = await sam.post(`/crm/${code}/items/${item}/move`, { stage_id: stages[1].id });
ok('move stage', r.status === 200 && r.data.stage_name === 'In progress');
r = await sam.get(`/crm/company/items/${co}`);
ok('shows on its company', r.data.related.some((x) => x.id === item && x.entity_type === T.id), r.data.related);

// ---- custom field, buyer, documents ----
r = await admin.post(`/crm/${code}/fields`, { label: 'Contract term', kind: 'number' });
ok('custom field', r.status === 200, r.data);
const fcode = r.data.code;
r = await sam.patch(`/crm/${code}/items/${item}`, { uf: { [fcode]: 24 } });
ok('custom value saved', r.status === 200 && r.data.uf[fcode] === 24, r.data.uf);
await sam.post(`/crm/items/${co}/requisites`, { country: 'DE', name: `Client GmbH ${stamp}`, fields: { vat: 'DE123456789' } });
r = await sam.get(`/crm/items/${item}/buyer`);
ok('buyer from its company', r.status === 200 && r.data.options.length === 1, r.data);
r = await admin.post('/crm/doc-templates', { name: `Contract doc ${stamp}`, engine: 'typst', entity_types: [code], doc_title: 'Contract', number_prefix: `C${stamp}-`, source: '#let d = json("data.json")\n= #d.doc.title #d.doc.number\n#d.client.legal_name — #d.record.title — #d.totals.total.text' });
ok('template for the type', r.status === 200 && r.data.entity_types.includes(T.id), r.data);
const tpl = r.data.id;
r = await sam.post(`/crm/items/${item}/documents`, { template_id: tpl });
ok('document generated', r.data.status === 'ready', r.data);
const pdf = Buffer.from(await (await sam.req('GET', `/files/${r.data.pdf_file_id}/content`, undefined, true)).arrayBuffer());
writeFileSync('nerp-api9.pdf', pdf);
const text = execSync('pdftotext nerp-api9.pdf -').toString();
ok('document data', text.includes(`Client GmbH ${stamp}`) && text.includes(`Master agreement ${stamp}`) && text.includes('14,280.00'), text);

// ---- automation and rights ----
r = await admin.post('/crm/robots', { category_id: cfg.categories[0].id, stage_id: stages[2].id, name: 'Done note', action: 'comment', params: { text: '{{type}} {{title}} done' } });
ok('robot on custom stage', r.status === 200, r.data);
await sam.post(`/crm/${code}/items/${item}/move`, { stage_id: stages[2].id });
const note = await waitFor(async () => (await sam.get(`/crm/items/${item}/timeline`)).data.items.find((e) => e.kind === 'comment' && e.body === `Contract ${stamp} Master agreement ${stamp} done`));
ok('robot ran', !!note);
r = await admin.get('/crm/permissions');
ok('rights matrix has the type', JSON.stringify(r.data).includes(`"${T.id}"`), Object.keys(r.data));
const roleId = r.data.roles.find((x) => !x.full_access).id;
const m = r.data.matrix[roleId] ?? {};
m[T.id] = { ...(m[T.id] ?? {}), read: 'own' };
const entries = Object.entries(m).flatMap(([et, acts]: [string, any]) => Object.entries(acts).map(([a, s]) => ({ entity_type: Number(et), action: a, scope: s })));
r = await admin.put(`/crm/permissions/${roleId}`, { entries });
ok('restrict read to own', r.status === 200, r.data);
ok('other employee no longer sees it', (await ivy.get(`/crm/${code}/items/${item}`)).status === 404);

// ---- update & delete ----
r = await admin.patch(`/crm/types/${code}`, { name_plural: `Agreements ${stamp}`, icon: 'briefcase' });
ok('rename keeps code', r.status === 200 && r.data.code === code && r.data.name_plural === `Agreements ${stamp}`);
ok('built-ins cannot be changed', (await admin.patch('/crm/types/deal', { name: 'x' })).status === 400);
r = await admin.del(`/crm/types/${code}`);
ok('delete blocked while records exist', r.status === 400 && /records/.test(r.data.error.message), r.data);
await sam.del(`/crm/${code}/items/${item}`);
const bin = (await admin.get('/crm/recycle-bin')).data;
ok('recycle bin lists it', bin.page.items.some((x) => x.id === item));
await admin.del(`/crm/recycle-bin/${item}`);
r = await admin.del(`/crm/types/${code}`);
ok('delete type', r.status === 200, r.data);
ok('gone from config', !(await admin.get('/crm/config')).data.types.some((t) => t.code === code));
r = await admin.get(`/crm/doc-templates/${tpl}`);
ok('template no longer offered for it', !r.data.entity_types.includes(T.id), r.data);
await admin.del(`/crm/doc-templates/${tpl}`);
await admin.del(`/crm/types/${reservedType}`);

console.log(`\n${pass} passed, ${fail} failed`);
await cleanupRun(stamp);
process.exit(fail ? 1 : 0);
