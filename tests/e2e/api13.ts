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
// CRM → Documents: the list across records, filters, permissions, ZIP download.
const tpls = (await admin.get('/crm/doc-templates')).data.items;
const tpl = (k) => tpls.find((t) => t.builtin_key === k);
ok('nine example invoices seeded', ['typst', 'latex', 'office'].every((e) => ['classic', 'modern', 'minimal'].every((s) => tpl(`${e}.invoice.${s}`)?.is_active)));

let r: any = await sam.post('/crm/company/items', { company_name: `Docs Co ${stamp}` });
const company = r.data.id;
r = await sam.post('/crm/deal/items', { title: `Docs deal ${stamp}`, company_id: company, currency: 'EUR', opportunity: '1200' });
const deal = r.data.id;
async function generate(item, key, who = sam) {
  const res = await who.post(`/crm/items/${item}/documents`, { template_id: tpl(key).id });
  let d = res.data;
  for (let i = 0; i < 120 && d.status === 'pending'; i++) { await sleep(500); d = (await who.get(`/crm/documents/${d.id}`)).data; }
  return d;
}
const docs: any[] = [];
for (const k of ['typst.invoice.classic', 'typst.invoice.modern', 'latex.invoice.minimal', 'office.invoice.modern', 'typst.quote']) docs.push(await generate(deal, k));
ok('documents generated from examples', docs.every((d) => d.status === 'ready' && d.pdf_file_id), docs.map((d) => [d.template_name, d.status, d.error]));

r = await sam.get(`/crm/documents?q=${encodeURIComponent(`Docs deal ${stamp}`)}`);
ok('list finds them by record title', r.status === 200 && r.data.total === 5, r.data);
const row = r.data.items.find((d) => d.id === docs[0].id);
ok('row has record, client, amount', row && row.type_code === 'deal' && row.item_title === `Docs deal ${stamp}` && row.client === `Docs Co ${stamp}` && +row.amount === 1200 && row.currency === 'EUR' && row.kind === 'Invoice', row);
ok('newest first', r.data.items[0].id === docs.at(-1).id);
r = await sam.get(`/crm/documents?q=${encodeURIComponent(`Docs Co ${stamp}`)}&kind=Invoice`);
ok('search by company + kind filter', r.data.total === 4 && r.data.items.every((d) => d.kind === 'Invoice'), r.data.items.map((d) => d.kind));
ok('kinds offered', r.data.kinds.includes('Invoice') && r.data.kinds.includes('Quote'));
r = await sam.get(`/crm/documents?q=${encodeURIComponent(docs[2].number)}`);
ok('search by number', r.data.total === 1 && r.data.items[0].id === docs[2].id, r.data);
const today = new Date().toISOString().slice(0, 10);
ok('date range includes today', (await sam.get(`/crm/documents?q=${encodeURIComponent(`Docs deal ${stamp}`)}&from=${today}&to=${today}`)).data.total === 5);
ok('date range excludes past', (await sam.get(`/crm/documents?q=${encodeURIComponent(`Docs deal ${stamp}`)}&to=2020-01-01`)).data.total === 0);
ok('type filter', (await sam.get(`/crm/documents?q=${encodeURIComponent(`Docs deal ${stamp}`)}&type=lead`)).data.total === 0);
ok('status filter', (await sam.get(`/crm/documents?q=${encodeURIComponent(`Docs deal ${stamp}`)}&status=failed`)).data.total === 0);
ok('bad date rejected', (await sam.get('/crm/documents?from=yesterday')).status === 400);
ok('paging', (await sam.get(`/crm/documents?q=${encodeURIComponent(`Docs deal ${stamp}`)}&limit=2&offset=4`)).data.items.length === 1);
ok('wildcards are literal', (await sam.get('/crm/documents?q=%25%25%25_')).data.total === 0);

// ZIP
let res = await sam.req('POST', '/crm/documents/zip', { ids: docs.map((d) => d.id) }, true);
const buf = Buffer.from(await res.arrayBuffer());
ok('zip download', res.status === 200 && res.headers.get('content-type')! === 'application/zip' && /attachment; filename="documents-/.test(res.headers.get('content-disposition')!) && buf.subarray(0, 2).toString() === 'PK', res.status);
const names = docs.map((d) => `${d.number}.pdf`);
ok('zip holds one PDF per document, named by number', names.every((n) => buf.includes(Buffer.from(n))) && buf.includes(Buffer.from('%PDF-')), names);
ok('zip: empty rejected', (await sam.post('/crm/documents/zip', { ids: [] })).status === 400);
ok('zip: too many rejected', (await sam.post('/crm/documents/zip', { ids: Array.from({ length: 201 }, () => crypto.randomUUID()) })).status === 400);

// Permissions: with read=own on deals, a colleague sees neither the list rows nor the ZIP.
const empRole = (await admin.get('/roles')).data.find((x) => x.code === 'employee');
const perms = (await admin.get('/crm/permissions')).data;
const original = Object.entries(perms.matrix[empRole.id]).flatMap(([t, acts]: [string, any]) => Object.entries(acts).map(([action, scope]) => ({ entity_type: +t, action, scope })));
try {
  await admin.put(`/crm/permissions/${empRole.id}`, { entries: original.map((e) => (e.entity_type === 2 && e.action === 'read' ? { ...e, scope: 'own' } : e)) });
  r = await ivy.get(`/crm/documents?q=${encodeURIComponent(`Docs deal ${stamp}`)}`);
  ok('read=own: colleague does not see the documents', r.status === 200 && r.data.total === 0, r.data);
  ok('read=own: owner still does', (await sam.get(`/crm/documents?q=${encodeURIComponent(`Docs deal ${stamp}`)}`)).data.total === 5);
  ok('read=own: colleague cannot zip them', (await ivy.post('/crm/documents/zip', { ids: docs.map((d) => d.id) })).status === 400);
  // Ivy's own deal is visible to Ivy.
  const own = (await ivy.post('/crm/deal/items', { title: `Ivy deal ${stamp}`, currency: 'EUR' })).data.id;
  const d = await generate(own, 'typst.invoice.minimal', ivy);
  r = await ivy.get(`/crm/documents?q=${encodeURIComponent(stamp)}`);
  ok('read=own: colleague sees own documents only', r.data.total === 1 && r.data.items[0].id === d.id, r.data.items.map((x) => x.item_title));
  await admin.del(`/crm/deal/items/${own}`);
} finally {
  await admin.put(`/crm/permissions/${empRole.id}`, { entries: original });
}
ok('permissions restored', (await ivy.get(`/crm/documents?q=${encodeURIComponent(`Docs deal ${stamp}`)}`)).data.total === 5);
r = await ivy.get(`/crm/documents?q=${encodeURIComponent(`Docs deal ${stamp}`)}`);
await admin.del(`/crm/deal/items/${deal}`);
ok('deleted records drop out', (await sam.get(`/crm/documents?q=${encodeURIComponent(`Docs deal ${stamp}`)}`)).data.total === 0);
await admin.del(`/crm/company/items/${company}`);
console.log(`\n${pass} passed, ${fail} failed`);
await cleanupRun(stamp);
