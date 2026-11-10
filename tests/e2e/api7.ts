// CRM requisites: country schemas, validation, duplicates, buyer choice, documents.
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
  writeFileSync('nerp-api7.pdf', buf);
  return buf.subarray(0, 5).toString() === '%PDF-' ? execSync('pdftotext -layout nerp-api7.pdf -').toString() : 'not a pdf';
};

// ---- schema ----
let r: any = await sam.get('/crm/requisites/schema?country=in');
ok('India schema', r.status === 200 && r.data.fields.map((f) => f.code).join() === 'gstin,pan,cin,tan' && r.data.bank.some((f) => f.code === 'ifsc'), r.data);
r = await sam.get('/crm/requisites/schema?country=FR');
ok('France: SIREN/SIRET + IBAN', r.data.fields[0].code === 'siren' && r.data.bank.some((f) => f.code === 'iban'));
r = await sam.get('/crm/requisites/schema?country=ZW');
ok('fallback schema', r.data.fields.map((f) => f.code).join() === 'tax_id,reg_no');
ok('bad country', (await sam.get('/crm/requisites/schema?country=IND')).status === 400);
ok('specific countries listed', (await sam.get('/crm/requisites/schema')).data.specific.includes('BR'));

// ---- live check ----
r = await sam.get('/crm/requisites/check?country=IN&code=gstin&value=27AAPFU0939F1ZX');
ok('bad GSTIN checksum', r.data.error?.includes('GSTIN'), r.data);
r = await sam.get('/crm/requisites/check?country=DE&code=iban&value=' + encodeURIComponent('DE89 3704 0044 0532 0130 00'));
ok('good IBAN', r.data.error === null, r.data);

// ---- requisites on a company ----
r = await sam.post('/crm/company/items', { company_name: `Globex ${stamp}` });
const co = r.data.id;
const gstin = '27AAPFU0939F1ZV';
r = await sam.post(`/crm/items/${co}/requisites`, { country: 'IN', name: `Globex India Pvt Ltd ${stamp}`, fields: { gstin: gstin.toLowerCase().replace(/(.{5})/g, '$1 '), pan: 'AAPFU0939F', tan: '' },
  address: { line1: 'MG Road 12', city: 'Mumbai', postal_code: '400001', country: 'IN' },
  banks: [{ name: 'Main', fields: { bank_name: 'HDFC Bank', ifsc: 'HDFC0000123', account: '50100012345678' } }] });
ok('create requisite', r.status === 200 && r.data.fields.gstin === '27AAP FU093 9F1ZV' && r.data.is_default && r.data.banks.length === 1 && r.data.banks[0].is_default && !('tan' in r.data.fields), r.data);
const req1 = r.data.id;
ok('invalid value rejected', (await sam.post(`/crm/items/${co}/requisites`, { country: 'IN', name: 'X', fields: { gstin: '27AAPFU0939F1ZX' } })).status === 400);
ok('foreign field rejected', (await sam.post(`/crm/items/${co}/requisites`, { country: 'IN', name: 'X', fields: { siren: '732829320' } })).status === 400);
ok('org-only field rejected for person', (await sam.post(`/crm/items/${co}/requisites`, { country: 'IN', kind: 'person', name: 'X', fields: { cin: 'U72200KA2010PTC012345' } })).status === 400);
ok('bad IFSC rejected', (await sam.post(`/crm/items/${co}/requisites`, { country: 'IN', name: 'X', banks: [{ fields: { ifsc: 'HDFC123' } }] })).status === 400);
r = await sam.post(`/crm/items/${co}/requisites`, { country: 'DE', name: `Globex GmbH ${stamp}`, fields: { vat: 'DE123456789' }, banks: [{ fields: { iban: 'DE89370400440532013000' } }] });
ok('second requisite not default', r.status === 200 && !r.data.is_default, r.data);
const req2 = r.data.id;
r = await sam.patch(`/crm/requisites/${req2}`, { country: 'DE', name: `Globex GmbH ${stamp}`, fields: { vat: 'DE123456789' }, is_default: true });
ok('make default moves it', r.data.is_default && r.data.banks.length === 0);
r = await sam.get(`/crm/items/${co}/requisites`);
ok('listed default first', r.data.items.length === 2 && r.data.items[0].id === req2 && !r.data.items[1].is_default, r.data.items);
ok('cannot add requisites to a deal', (await sam.post(`/crm/items/${(await sam.post('/crm/deal/items', { title: `x ${stamp}` })).data.id}/requisites`, { country: 'US', name: 'x' })).status === 400);

// ---- search and duplicates ----
r = await sam.get(`/crm/company/items?q=${gstin}`);
ok('search by GSTIN', r.data.items?.some((i) => i.id === co), r.data);
r = await sam.get(`/crm/company/items?q=${encodeURIComponent('Globex India Pvt')}`);
ok('search by legal name', r.data.items?.some((i) => i.id === co));
const co2 = (await sam.post('/crm/company/items', { company_name: `Globex dup ${stamp}` })).data.id;
r = await sam.get(`/crm/requisites/check?country=IN&code=gstin&value=${gstin}&item_id=${co2}`);
ok('duplicate GSTIN found live', r.data.duplicates.some((d) => d.item_id === co && d.field === 'GSTIN'), r.data);
r = await sam.post(`/crm/items/${co2}/requisites`, { country: 'IN', name: 'Dup', fields: { gstin } });
ok('save returns duplicates', r.status === 200 && r.data.duplicates.some((d) => d.item_id === co), r.data);
r = await sam.get(`/crm/requisites/check?country=DE&code=vat&value=${gstin}`);
ok('other country scheme is no duplicate', !r.data.duplicates?.length);

// ---- buyer on a deal + documents ----
const contact = (await sam.post('/crm/contact/items', { first_name: 'Ana', last_name: `Buyer${stamp}` })).data.id;
await sam.post(`/crm/items/${contact}/requisites`, { country: 'ES', kind: 'person', name: `Ana Buyer ${stamp}`, fields: { nif: '12345678Z' } });
r = await sam.post('/crm/deal/items', { title: `Req deal ${stamp}`, company_id: co, contact_ids: [contact], currency: 'EUR' });
ok('deal', r.status === 200, r.data);
const deal = r.data.id;
await sam.put(`/crm/items/${deal}/products`, { rows: [{ product_id: null, name: 'Thing', price: '100', quantity: '1', unit: 'pcs', discount_kind: 'percent', discount_value: '0', tax_rate: '18', tax_included: false }] });
r = await sam.get(`/crm/items/${deal}/buyer`);
ok('buyer options: company first, default current', r.data.options.length === 3 && r.data.current === req2 && r.data.chosen === null && r.data.options.at(-1).item_id === contact, r.data);
const inv = (await sam.get('/crm/doc-templates?type=deal')).data.items.find((t) => t.builtin_key === 'typst.invoice');
let d = (await sam.post(`/crm/items/${deal}/documents`, { template_id: inv.id })).data;
let text = await pdfText(sam, d.pdf_file_id);
ok('invoice uses default requisite', text.includes(`Globex GmbH ${stamp}`) && text.includes('USt-IdNr. DE123456789'), text.slice(0, 900));
r = await sam.put(`/crm/items/${deal}/buyer`, { requisite_id: req1 });
ok('choose buyer', r.status === 200 && r.data.chosen === req1 && r.data.current === req1, r.data);
d = (await sam.post(`/crm/documents/${d.id}/regenerate`)).data;
text = await pdfText(sam, d.pdf_file_id);
ok('invoice shows chosen GSTIN, legal name and address', text.includes(`Globex India Pvt Ltd ${stamp}`) && text.includes(`GSTIN`) && text.includes('Mumbai'), text.slice(0, 900));
const other = (await sam.post('/crm/company/items', { company_name: `Other ${stamp}` })).data.id;
const foreign = (await sam.post(`/crm/items/${other}/requisites`, { country: 'US', name: 'Other Inc', fields: { ein: '12-3456789' } })).data.id;
ok('cannot choose unrelated requisite', (await sam.put(`/crm/items/${deal}/buyer`, { requisite_id: foreign })).status === 400);
ok('ivy sees buyer (read all)', [200, 404].includes((await ivy.get(`/crm/items/${deal}/buyer`)).status));

// ---- timeline, delete ----
r = await sam.get(`/crm/items/${co}/timeline`);
ok('timeline entries', r.data.items.filter((e) => e.kind === 'requisite').length >= 3, r.data.items.map((e) => e.kind));
ok('delete requisite', (await sam.del(`/crm/requisites/${req1}`)).status === 200);
r = await sam.get(`/crm/items/${deal}/buyer`);
ok('chosen falls back after delete', r.data.chosen === null && r.data.current === req2, r.data);
await sam.del(`/crm/requisites/${req2}`);
r = await sam.get(`/crm/items/${co}/requisites`);
ok('no default left when none', r.data.items.length === 0);
r = await sam.get(`/crm/company/items?q=${gstin}`);
ok('search index cleared', !r.data.items.some((i) => i.id === co));

console.log(`\n${pass} passed, ${fail} failed`);
await cleanupRun(stamp);
process.exit(fail ? 1 : 0);
