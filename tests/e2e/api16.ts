// Quotes and invoices: built-in types, made from deals and quotes, linked lists, payment,
// documents, analytics, and protection of the built-in types.
import { cleanupRun } from './cleanup.ts';
import { Client } from './lib.ts';
const B = 'http://localhost:8080/api';
let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 700)); };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const stamp = Date.now().toString(36);
const admin = new Client(); await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const waitFor = async (fn, ms = 20000) => { const end = Date.now() + ms; let v; while (Date.now() < end) { v = await fn(); if (v) return v; await sleep(300); } return v; };
const made: Record<string, any[]> = { deal: [], quote: [], invoice: [], contact: [], company: [], lead: [] };

// ---- the types ----
let cfg = (await admin.get('/crm/config')).data;
const qt = cfg.types.find((t) => t.kind === 'quote'), it = cfg.types.find((t) => t.kind === 'invoice');
ok('quote and invoice types exist', qt?.code === 'quote' && it?.code === 'invoice' && qt.has_amount && it.has_client && !qt.is_system, [qt, it]);
const qs = Object.fromEntries(qt.categories[0].stages.map((s) => [s.name, s])), is = Object.fromEntries(it.categories[0].stages.map((s) => [s.name, s]));
ok('their stages', qs.Draft && qs.Sent && qs.Accepted?.semantics === 'success' && qs.Declined?.semantics === 'failure' && is.Paid?.semantics === 'success' && is.Cancelled, [Object.keys(qs), Object.keys(is)]);
ok('date fields named for them', qt.system_fields.find((f) => f.code === 'close_date')?.label === 'Valid until' && it.system_fields.find((f) => f.code === 'close_date')?.label === 'Due date');
ok('cannot delete them', (await admin.del('/crm/types/invoice')).status === 400);
ok('cannot drop their amount', (await admin.patch('/crm/types/quote', { has_amount: false })).status === 400);
let r: any = await admin.patch('/crm/types/quote', { name_plural: 'Proposals' });
ok('can rename them', r.status === 200 && r.data.name_plural === 'Proposals' && r.data.kind === 'quote', r.data);
await admin.patch('/crm/types/quote', { name_plural: 'Quotes' });
r = await admin.post('/crm/types', { name: 'Invoice', name_plural: 'Invoices' });
ok('a smart process cannot take their code', r.status === 200 && !['invoice', 'invoices'].includes(r.data.code), r.data);
if (r.data?.code) await admin.del(`/crm/types/${r.data.code}`);
ok('templates offered for invoices', (await admin.get('/crm/doc-templates')).data.items.some((t) => t.entity_types.includes(it.id) && t.engine === 'typst'));

// ---- a deal with products ----
const company = (await admin.post('/crm/company/items', { company_name: `Billing Co ${stamp}` })).data; made.company.push(company.id);
const contact = (await admin.post('/crm/contact/items', { first_name: 'Bea', last_name: `Bill ${stamp}`, company_id: company.id })).data; made.contact.push(contact.id);
const deal = (await admin.post('/crm/deal/items', { title: `Fit-out ${stamp}`, company_id: company.id, contact_ids: [contact.id], currency: cfg.base_currency })).data; made.deal.push(deal.id);
r = await admin.put(`/crm/items/${deal.id}/products`, { rows: [{ name: 'Desk', price: '400', quantity: '3' }, { name: 'Chair', price: '150', quantity: '4' }] }).catch(() => null);
if (!r || r.status >= 400) r = await admin.req('PUT', `/crm/items/${deal.id}/products`, [{ name: 'Desk', price: '400', quantity: '3' }, { name: 'Chair', price: '150', quantity: '4' }]);
const d1 = (await admin.get(`/crm/deal/items/${deal.id}`)).data;
ok('deal with products', +d1.opportunity === 1800, [r?.status, r?.data, d1.opportunity]);

// ---- quote from the deal ----
r = await admin.post(`/crm/quote/items/from/${deal.id}`);
const quote = r.data; if (quote?.id) made.quote.push(quote.id);
const inDays = (n) => { const d = new Date(); d.setUTCDate(d.getUTCDate() + n); return d.toISOString().slice(0, 10); };
ok('quote made from the deal', r.status === 200 && quote.title === deal.title && quote.company_id === company.id && quote.contacts[0]?.id === contact.id && +quote.opportunity === 1800 && quote.currency === cfg.base_currency, r.data);
ok('quote links back and is valid for 30 days', quote.source_item?.id === deal.id && quote.close_date === inDays(30) && quote.stage_id === qs.Draft.id, [quote.source_item, quote.close_date]);
const qp = (await admin.get(`/crm/items/${quote.id}/products`)).data;
const qrows = qp.rows ?? qp.items ?? qp;
ok('quote has the products', Array.isArray(qrows) && qrows.length === 2 && qrows.some((p) => p.row.name === 'Desk' && +p.row.quantity === 3), qp);

// ---- invoice from the quote ----
r = await admin.post(`/crm/invoice/items/from/${quote.id}`);
const inv = r.data; if (inv?.id) made.invoice.push(inv.id);
ok('invoice made from the quote, due in 14 days', r.status === 200 && inv.source_item?.id === quote.id && inv.close_date === inDays(14) && +inv.opportunity === 1800, r.data);
r = await admin.post(`/crm/invoice/items/from/${deal.id}`);
const inv2 = r.data; if (inv2?.id) made.invoice.push(inv2.id);
ok('invoice made straight from the deal', r.status === 200 && inv2.source_item?.id === deal.id, r.data);

// ---- what cannot be made from what ----
ok('no quote from a contact', (await admin.post(`/crm/quote/items/from/${contact.id}`)).status === 400);
ok('no quote from an invoice', (await admin.post(`/crm/quote/items/from/${inv.id}`)).status === 400);
ok('no deal through this route', (await admin.post(`/crm/deal/items/from/${quote.id}`)).status === 400);
ok('unknown source', (await admin.post(`/crm/invoice/items/from/00000000-0000-0000-0000-000000000000`)).status === 404);

// ---- the deal lists them ----
let dd = (await admin.get(`/crm/deal/items/${deal.id}`)).data;
ok('deal lists its quote and both invoices', dd.children.length === 3 && [quote.id, inv.id, inv2.id].every((id) => dd.children.some((c) => c.id === id)), dd.children);
ok('children carry stage and due date', dd.children.find((c) => c.id === inv.id)?.stage_semantics === 'process' && dd.children.find((c) => c.id === inv.id).close_date === inDays(14));
ok('quote lists its invoice', (await admin.get(`/crm/quote/items/${quote.id}`)).data.children.some((c) => c.id === inv.id));
const tl = (await admin.get(`/crm/items/${deal.id}/timeline?limit=50`)).data.items;
ok('deal timeline notes them', tl.filter((e) => e.kind === 'linked').length === 2 && tl.some((e) => e.kind === 'linked' && e.data.code === 'quote'), tl.map((e) => e.kind));

// ---- payment ----
r = await admin.patch(`/crm/invoice/items/${inv.id}`, { stage_id: is.Paid.id });
ok('invoice paid', r.status === 200 && r.data.is_closed && r.data.closed_at, r.data);
r = await admin.patch(`/crm/quote/items/${quote.id}`, { stage_id: qs.Accepted.id });
ok('quote accepted', r.status === 200 && r.data.stage_semantics === 'success');
dd = (await admin.get(`/crm/deal/items/${deal.id}`)).data;
ok('paid shows on the deal', dd.children.find((c) => c.id === inv.id)?.stage_semantics === 'success');
r = await admin.patch(`/crm/invoice/items/${inv2.id}`, { close_date: inDays(-3) });
ok('due date editable', r.status === 200 && r.data.close_date === inDays(-3));
ok('filter overdue invoices', (await admin.get(`/crm/invoice/items?close_date_to=${inDays(-1)}&q=${encodeURIComponent('Fit-out ' + stamp)}`)).status === 200);

// ---- documents, board, analytics, automation ----
const tpl = (await admin.get('/crm/doc-templates')).data.items.find((t) => t.entity_types.includes(it.id) && t.engine === 'typst' && t.is_active);
r = await admin.post(`/crm/items/${inv.id}/documents`, { template_id: tpl.id });
const doc = r.data;
ok('invoice document generated', r.status === 200 && (await waitFor(async () => (await admin.get(`/crm/items/${inv.id}/documents`)).data.items?.find((d) => d.status === 'ready'))), r.data);
r = await admin.get(`/crm/invoice/kanban?per_column=20`);
ok('invoice board', r.status === 200 && r.data.columns.length === 4, r.data);
r = await admin.get(`/crm/analytics/invoice?from=${inDays(-1)}&to=${inDays(1)}`);
ok('invoice analytics: paid counts as won', r.status === 200 && r.data.kpi.current.won_count >= 1, r.data.kpi);
r = await admin.post('/crm/robots', { category_id: it.categories[0].id, stage_id: is.Sent.id, name: `Chase ${stamp}`, action: 'comment', params: { text: 'Sent' }, timing: 'field', timing_field: 'close_date', offset_minutes: 1440 });
ok('robots timed from the due date', r.status === 200 && r.data.timing_field === 'close_date', r.data);
if (r.data?.id) await admin.del(`/crm/robots/${r.data.id}`);

// ---- cleanup ----
for (const [code, ids] of Object.entries(made)) for (const id of ids) { await admin.del(`/crm/${code}/items/${id}`); await admin.del(`/crm/recycle-bin/${id}`); }
void doc;
console.log(`\n${pass} passed, ${fail} failed`);
await cleanupRun(stamp);
process.exit(fail ? 1 : 0);
