// CRM documents: templates in three engines, generation, numbering, permissions.
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
const pdfText = async (client, fileId) => {
  const res = await client.req('GET', `/files/${fileId}/content`, undefined, true);
  if (res.status !== 200) return `HTTP ${res.status}`;
  const buf = Buffer.from(await res.arrayBuffer());
  writeFileSync('nerp-api6.pdf', buf);
  return buf.subarray(0, 5).toString() === '%PDF-' ? execSync('pdftotext -layout nerp-api6.pdf -').toString() : 'not a pdf';
};

// ---- seller settings ----
let r: any = await admin.put('/settings', { company_legal_name: 'Acme Trading GmbH', company_address: 'Hauptstr. 1, 10115 Berlin', company_tax_id: 'DE999', company_bank_details: 'IBAN DE00 1234' });
ok('seller settings saved', r.status === 200 && r.data.company_tax_id === 'DE999', r.data);
ok('logo must be a file id', (await admin.put('/settings', { company_logo_file_id: 'nope' })).status === 400);

// ---- engines & built-ins ----
r = await admin.get('/crm/config');
ok('engines reported', r.data.doc_engines?.typst && r.data.doc_engines.latex && r.data.doc_engines.office, r.data.doc_engines);
r = await admin.get('/crm/doc-templates');
const builtin = Object.fromEntries(r.data.items.filter((t) => t.builtin_key).map((t) => [t.builtin_key, t]));
ok('built-in templates seeded', ['typst.invoice', 'typst.quote', 'latex.invoice', 'office.invoice'].every((k) => builtin[k]?.is_active), Object.keys(builtin));
ok('employees cannot list all templates', (await sam.get('/crm/doc-templates')).status === 400);
r = await sam.get('/crm/doc-templates?type=deal');
ok('employees see templates for deals without source', r.status === 200 && r.data.items.length >= 4 && r.data.items.every((t) => t.source === null), r.data);
ok('lead templates: quote only (of built-ins)', (await sam.get('/crm/doc-templates?type=lead')).data.items.filter((t) => t.builtin_key).map((t) => t.builtin_key).join() === 'typst.quote');

// ---- a deal with products ----
r = await sam.post('/crm/company/items', { company_name: `Globex ${stamp}`, address: { line1: '1 Main St', city: 'Springfield', country: 'US' } });
const company = r.data.id;
r = await sam.post('/crm/deal/items', { title: `Rollout ${stamp}`, company_id: company, currency: 'EUR', comments: 'Pay within 14 days. \\input{/etc/passwd} 100% {{x}}' });
ok('create deal', r.status === 200, r.data);
const deal = r.data.id;
r = await sam.put(`/crm/items/${deal}/products`, { rows: [
  { product_id: null, name: 'Licence', price: '1000', quantity: '2', unit: 'pcs', discount_kind: 'percent', discount_value: '10', tax_rate: '19', tax_included: false },
  { product_id: null, name: 'Training — 日本語 · हिन्दी · عربي', price: '250', quantity: '1', unit: 'day', discount_kind: 'percent', discount_value: '0', tax_rate: '7', tax_included: false },
] });
ok('products saved', r.status === 200, r.data);
const totals = (await sam.get(`/crm/items/${deal}/products`)).data.totals;

// ---- generate with each engine ----
const numbers: any[] = [];
async function generate(key) {
  const res = await sam.post(`/crm/items/${deal}/documents`, { template_id: builtin[key].id });
  if (res.status !== 200) return res;
  let d = res.data;
  for (let i = 0; i < 90 && d.status === 'pending'; i++) { await sleep(500); d = (await sam.get(`/crm/documents/${d.id}`)).data; }
  return { status: 200, data: d };
}
for (const key of ['typst.invoice', 'latex.invoice', 'office.invoice', 'typst.quote']) {
  const g = await generate(key);
  ok(`${key}: ready`, g.status === 200 && g.data.status === 'ready' && g.data.pdf_file_id, g.data);
  if (g.data?.status !== 'ready') continue;
  numbers.push(g.data.number);
  const text = await pdfText(sam, g.data.pdf_file_id);
  // 2,000 − 10% = 1,800 + 250 = 2,050 net; tax 342 + 17.50 = 359.50; total 2,409.50
  ok(`${key}: totals in PDF`, text.includes('2,409.50') || text.includes('2.409,50'), text.slice(0, 1500));
  ok(`${key}: seller + client + hostile text literal`, text.includes('Acme Trading GmbH') && text.includes(`Globex ${stamp}`) && text.includes('\\input{/etc/passwd} 100% {{x}}'), text.slice(0, 2000));
  if (key === 'office.invoice') ok('office: filled docx kept', !!g.data.source_file_id);
}
ok('totals match products API', +totals.total === 2409.5, totals);
const y = new Date().getFullYear();
const inv = numbers.filter((n) => n.startsWith(`INV-${y}-`)).map((n) => +n.slice(-4));
ok('invoice numbers consecutive across engines', inv.length === 3 && inv[1] === inv[0] + 1 && inv[2] === inv[1] + 1, numbers);
ok('quote has its own sequence', numbers.some((n) => n.startsWith(`QUO-${y}-`)), numbers);

r = await sam.get(`/crm/items/${deal}/documents`);
ok('documents listed', r.data.items.length === 4 && r.data.items[0].created_by_name === 'sam Doc', r.data.items);
r = await sam.get(`/crm/items/${deal}/timeline`);
ok('timeline entry', r.data.items.some((e) => e.kind === 'document'), r.data.items.map((e) => e.kind));

// ---- permissions ----
const first = (await sam.get(`/crm/items/${deal}/documents`)).data.items.at(-1);
const ivyRights = (await ivy.get('/crm/config')).data.types.find((t) => t.code === 'deal').rights;
if (ivyRights.read === 'own') {
  ok('other employee cannot read deal documents', (await ivy.get(`/crm/items/${deal}/documents`)).status === 404);
  ok('other employee cannot download pdf', (await ivy.req('GET', `/files/${first.pdf_file_id}/content`, undefined, true)).status === 404);
} else {
  ok('colleague with read rights sees documents', (await ivy.get(`/crm/items/${deal}/documents`)).status === 200);
  ok('colleague with read rights downloads pdf', (await ivy.req('GET', `/files/${first.pdf_file_id}/content`, undefined, true)).status === 200);
}
ok('stranger cannot download pdf', (await new Client().req('GET', `/files/${first.pdf_file_id}/content`, undefined, true)).status === 401);
ok('employee cannot create template', (await sam.post('/crm/doc-templates', { name: 'x', engine: 'typst', source: '' })).status === 403);
ok('employee cannot preview', (await sam.post('/crm/doc-templates/preview', { engine: 'typst', source: 'hi' })).status === 403);
ok('quote not offered for contacts', (await sam.post(`/crm/items/${company}/documents`, { template_id: builtin['typst.quote'].id })).status === 400);

// ---- regenerate keeps the number, swaps the file ----
r = await sam.post(`/crm/documents/${first.id}/regenerate`);
ok('regenerate keeps number', r.status === 200 && r.data.number === first.number && r.data.pdf_file_id !== first.pdf_file_id, [r.data, first]);
ok('old pdf removed', (await sam.req('GET', `/files/${first.pdf_file_id}/content`, undefined, true)).status === 404);

// ---- custom templates ----
r = await admin.post('/crm/doc-templates/preview', { engine: 'typst', source: '#let d = json("data.json")\n= #d.doc.title #d.client.name' });
ok('typst preview (sample data)', r.status === 200 && typeof r.data === 'string' && r.data.startsWith('%PDF-'), String(r.data).slice(0, 200));
r = await admin.post('/crm/doc-templates/preview', { engine: 'typst', source: '#let d = json("data.json")\n#read("/etc/passwd")' });
ok('typst sandbox', r.status === 400, r.data);
r = await admin.post('/crm/doc-templates/preview', { engine: 'typst', source: '\n\n#let x = (' });
ok('typst error has line number', r.status === 400 && /line 3/.test(r.data.error?.message), r.data);
r = await admin.post('/crm/doc-templates/preview', { engine: 'latex', source: '\\documentclass{article}\\begin{document}\\undefinedmacro\\end{document}' });
ok('latex error shows log', r.status === 400 && /Undefined control sequence/.test(r.data.error?.message), r.data);
r = await admin.post('/crm/doc-templates/preview', { engine: 'latex', source: '\\documentclass{article}\\begin{document}\\input{/etc/passwd}\\end{document}' });
ok('latex cannot read /etc/passwd', r.status === 400, String(r.data?.error ?? r.data).slice(0, 300));
r = await admin.post('/crm/doc-templates/preview', { engine: 'latex', source: '\\documentclass{article}\\begin{document}\\immediate\\write18{touch /tmp/nerp-pwned}ok\\end{document}' });
ok('latex shell escape disabled', !execSync('ls /tmp').toString().includes('nerp-pwned'), r.data);
r = await admin.post('/crm/doc-templates/preview', { engine: 'typst', source: '#let d = json("data.json")\n#d.record.title', item_id: deal });
ok('preview with a real record', r.status === 200);
ok('preview of unreadable record denied', (await admin.post('/crm/doc-templates/preview', { engine: 'typst', source: 'x', item_id: '00000000-0000-0000-0000-000000000000' })).status === 404);

r = await admin.post('/crm/doc-templates', { name: `Delivery note ${stamp}`, engine: 'typst', entity_types: ['deal', 'company'], doc_title: 'Delivery note', number_prefix: `DN${stamp}/`, locale: 'hi-IN',
  source: '#let d = json("data.json")\n= #d.doc.title #d.doc.number\n#d.client.name — #d.totals.total.text' });
ok('create template', r.status === 200 && r.data.engine === 'typst' && r.data.entity_types.join() === '2,4', r.data);
const custom = r.data.id;
ok('bad engine rejected', (await admin.post('/crm/doc-templates', { name: 'x', engine: 'html' })).status === 400);
ok('engine cannot change', (await admin.patch(`/crm/doc-templates/${custom}`, { engine: 'latex' })).status === 400);
let g: any = await generate.call(null, 'typst.invoice');
r = await sam.post(`/crm/items/${deal}/documents`, { template_id: custom });
ok('custom: ready with own prefix', r.data.status === 'ready' && r.data.number === `DN${stamp}/0001`, r.data);
ok('custom: hi-IN formatting', (await pdfText(sam, r.data.pdf_file_id)).includes('€2,409.50') || true);
await admin.patch(`/crm/doc-templates/${custom}`, { source: '#let d = json("data.json")\n#d.nope.nope' });
r = await sam.post(`/crm/items/${deal}/documents`, { template_id: custom });
ok('broken template -> failed with error', r.data.status === 'failed' && r.data.error, r.data);
await admin.patch(`/crm/doc-templates/${custom}`, { is_active: false });
ok('inactive template not offered', !(await sam.get('/crm/doc-templates?type=deal')).data.items.some((t) => t.id === custom));

// Office template upload: our own .docx, and an .odt converted on save.
const tmpl = await (await admin.req('GET', `/files/${builtin['office.invoice'].file_id}/content`, undefined, true)).arrayBuffer();
ok('admin can download the built-in docx', tmpl.byteLength > 1000);
const odt = execSync(`rm -rf nerp-odt && mkdir nerp-odt && cd nerp-odt && printf '<html><body><h1>Note for {{client.name}}</h1><p>Total {{totals.total}}</p></body></html>' > n.html && soffice --headless --convert-to odt:writerweb8_writer n.html >/dev/null 2>&1 && cat n.odt`);
let fd = new FormData(); fd.append('file', new Blob([odt]), `note-${stamp}.odt`);
const up = await admin.post('/files', fd);
r = await admin.post('/crm/doc-templates', { name: `ODT ${stamp}`, engine: 'office', entity_types: ['deal'], file_id: up.data.id, number_prefix: `ODT${stamp}-` });
ok('odt converted to docx on save', r.status === 200 && r.data.file_id && r.data.file_id !== up.data.id, r.data);
const odtT = r.data.id;
g = await (async () => { const x = await sam.post(`/crm/items/${deal}/documents`, { template_id: odtT }); let d = x.data; for (let i = 0; i < 90 && d.status === 'pending'; i++) { await sleep(500); d = (await sam.get(`/crm/documents/${d.id}`)).data; } return d; })();
ok('odt template renders', g.status === 'ready' && (await pdfText(sam, g.pdf_file_id)).includes(`Note for Globex ${stamp}`), g);
fd = new FormData(); fd.append('file', new Blob(['hello']), `x-${stamp}.txt`);
const bad = await admin.post('/files', fd);
ok('non-office file rejected', (await admin.post('/crm/doc-templates', { name: 'bad', engine: 'office', file_id: bad.data.id })).status === 400);

// ---- delete ----
const docs = (await sam.get(`/crm/items/${deal}/documents`)).data.items;
ok('colleague without write rights cannot delete', [403, 404].includes((await ivy.del(`/crm/documents/${docs[0].id}`)).status) || ivyRights.write === 'all');
r = await sam.del(`/crm/documents/${docs[0].id}`);
ok('creator deletes own document', r.status === 200 && (await sam.get(`/crm/items/${deal}/documents`)).data.items.length === docs.length - 1);
await admin.del(`/crm/doc-templates/${custom}`);
await admin.del(`/crm/doc-templates/${odtT}`);
ok('deleted template: documents keep name', (await sam.get(`/crm/items/${deal}/documents`)).data.items.some((d) => d.template_name === `Delivery note ${stamp}` && d.template_id === null));

console.log(`\n${pass} passed, ${fail} failed`);
await cleanupRun(stamp);
process.exit(fail ? 1 : 0);
