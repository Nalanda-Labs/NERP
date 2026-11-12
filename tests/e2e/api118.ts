// Credit notes and commissions.
//
// Credit notes: part of an invoice taken back with a reversing entry; the invoice is owed less,
// settles by payments and credit notes, and a credit note can be voided.
//
// Commissions: Sam's plans (10% of the sale, 12% once the month passes 1000; 5% more on
// widgets) earn as invoices are paid and give it back when a payment is voided or a paid
// invoice is credited; statements, tiers, locked months and the payroll export.
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { dbUrl } from '../db/cleanup.ts';
import pg from 'pg';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const s = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
const base = (await admin.get('/crm/config')).data.base_currency;
const today = new Date().toISOString().slice(0, 10);
const thisMonth = today.slice(0, 7);
const made: { items: string[]; payments: string[]; plans: string[]; months: string[]; products: string[] } = { items: [], payments: [], plans: [], months: [], products: [] };
const n = (v: unknown) => Number(v);

try {
  const ready = (await admin.get('/inventory/settings')).data.accounting_ready;
  if (!ready) {
    console.log('skipped: accounting is not set up');
  } else {
    const bank = (await admin.get('/accounting/accounts')).data.items.find((a: any) => a.subtype === 'bank' && a.is_active);
    await admin.post('/users', { email: `sam.${s}@acme.test`, password: 'password123', first_name: 'Sam', last_name: `Comm${s}` });
    const sam = new Client();
    const samUser = (await sam.post('/auth/login', { email: `sam.${s}@acme.test`, password: 'password123' })).data.user;
    const widget = (await admin.post('/crm/products', { name: `Widget ${s}`, price: '100' })).data;
    const service = (await admin.post('/crm/products', { name: `Fitting ${s}`, price: '300' })).data;
    made.products.push(widget.id, service.id);
    const client = (await admin.post('/crm/company/items', { company_name: `Buyer ${s}` })).data;

    // ---- plans ----
    ok('plans are for those who manage commissions', (await sam.post('/commissions/plans', { name: 'Mine', rate: '50' })).status === 403);
    ok('rates are percents', (await admin.post('/commissions/plans', { name: 'Bad', rate: '150' })).status === 400);
    let r = await admin.post('/commissions/plans', { name: `Base ${s}`, rate: '10', tiers: [{ from: '1000', rate: '12' }], user_ids: [samUser.id] });
    made.plans.push(r.data.id);
    r = await admin.post('/commissions/plans', { name: `Widgets ${s}`, rate: '5', product_ids: [widget.id], user_ids: [samUser.id] });
    made.plans.push(r.data.id);
    ok('plans saved', made.plans.length === 2 && r.data.product_ids[0] === widget.id, r.data);

    const invoice = async (title: string, rows: any[]) => {
      const i = (await admin.post('/crm/invoice/items', { title: `${title} ${s}`, company_id: client.id, currency: base, assigned_to: samUser.id })).data;
      made.items.push(i.id);
      await admin.put(`/crm/items/${i.id}/products`, { rows });
      const p = await admin.post(`/accounting/invoices/${i.id}/post`, { date: today });
      if (p.status !== 200) throw new Error(`post: ${JSON.stringify(p.data)}`);
      return i;
    };
    const pay = async (item: string, amount: string) => {
      const x = await admin.post('/accounting/payments', { direction: 'in', date: today, account_id: bank.id, amount, allocations: [{ doc_kind: 'invoice', doc_id: item, amount }] });
      if (x.data?.id) made.payments.push(x.data.id);
      return x;
    };
    const stmt = async (month = thisMonth) => (await sam.get(`/commissions/statement?month=${month}`)).data;

    // Invoice 1: 2 widgets (200) + fitting (300) = 500.
    const i1 = await invoice('One', [{ product_id: widget.id, name: 'Widget', price: '100', quantity: '2' }, { product_id: service.id, name: 'Fitting', price: '300', quantity: '1' }]);
    ok('nothing earned before money comes in', (await stmt()).entries.length === 0);
    const p1 = await pay(i1.id, '250.00');
    let st = await stmt();
    ok('half paid: 10% of half the sale, and 5% of half the widgets', n(st.net) === 30 && st.entries.length === 2 && st.entries.some((e: any) => n(e.amount) === 25 && /50% paid/.test(e.reason)) && st.entries.some((e: any) => n(e.amount) === 5), st);
    const p2 = await pay(i1.id, '250.00');
    st = await stmt();
    ok('fully paid: the rest', n(st.net) === 60 && st.earned === '60.00', st);
    await admin.post(`/accounting/payments/${p2.data.id}/void`, {});
    st = await stmt();
    ok('a voided payment takes its commission back', n(st.net) === 30 && n(st.taken_back) === -30 && st.entries.some((e: any) => /taken back/.test(e.reason)), st);

    // ---- credit notes ----
    ok('crediting more than the invoice is refused', (await admin.post(`/accounting/invoices/${i1.id}/credit-notes`, { amount: '600' })).status === 400);
    r = await admin.post(`/accounting/invoices/${i1.id}/credit-notes`, { amount: '250', reason: 'Fitting not needed' });
    ok('a credit note', r.status === 200 && r.data.number > 0 && n(r.data.invoice.credited) === 250 && n(r.data.invoice.balance) === 0 && r.data.invoice.status === 'credited', r.data);
    const cn1 = r.data.id;
    const entry = (await db.query("SELECT e.id, SUM(l.debit) AS d, SUM(l.credit) AS c FROM acc_credit_notes n JOIN acc_entries e ON e.id = n.entry_id JOIN acc_lines l ON l.entry_id = e.id WHERE n.id = $1 GROUP BY e.id", [cn1])).rows[0];
    ok('…with a balanced reversing entry for its share', entry && n(entry.d) === n(entry.c) && n(entry.d) === 250, entry);
    ok('…which owes nothing more, so commissions stand (half paid, half credited)', n((await stmt()).net) === 30);
    ok('an invoice with credit notes can\'t be voided', (await admin.post(`/accounting/invoices/${i1.id}/void`, {})).status === 400);
    r = await admin.post(`/accounting/credit-notes/${cn1}/void`, {});
    ok('voiding the credit note: owed again', r.status === 200 && n(r.data.credited) === 0 && n(r.data.balance) === 250 && r.data.status === 'open', r.data);

    // Invoice 2: a fitting at 400, paid, then 100 credited after payment.
    const i2 = await invoice('Two', [{ product_id: service.id, name: 'Fitting', price: '400', quantity: '1' }]);
    await pay(i2.id, '400.00');
    ok('invoice two earns 40', n((await stmt()).net) === 70);
    await admin.post(`/accounting/invoices/${i2.id}/credit-notes`, { amount: '100' });
    st = await stmt();
    ok('crediting a paid invoice takes back the commission on the credit', n(st.net) === 60 && st.entries.some((e: any) => n(e.amount) === -10 && /credited/.test(e.reason)), st.entries.map((e: any) => [e.amount, e.reason]));

    // ---- tiers: the month's sales so far are 250 + 300 = 550; 1000 more passes 1000 ----
    const i3 = await invoice('Three', [{ product_id: service.id, name: 'Fitting', price: '1000', quantity: '1' }]);
    await pay(i3.id, '1000.00');
    st = await stmt();
    ok('past the tier, the higher rate', st.entries.some((e: any) => e.invoice_id === i3.id && n(e.rate) === 12 && n(e.amount) === 120), st.entries.filter((e: any) => e.invoice_id === i3.id));

    // ---- access ----
    const other = (await admin.get('/users?limit=1')).data.items?.[0]?.id ?? '00000000-0000-0000-0000-000000000000';
    ok('one\'s own statement only', (await sam.get(`/commissions/statement?user=${other}`)).status === 403);
    ok('the team summary is for managers', (await sam.get('/commissions/summary')).status === 403);
    r = await admin.get(`/commissions/summary?month=${thisMonth}`);
    ok('the month\'s summary', r.data.people.some((p: any) => p.id === samUser.id && n(p.net) === 180), r.data.people?.find((p: any) => p.id === samUser.id));
    r = await admin.get(`/commissions/statement?user=${samUser.id}&month=${thisMonth}`);
    ok('…and anyone\'s statement for those who manage commissions', r.status === 200 && n(r.data.net) === 180);

    // ---- locking and payroll ----
    const csv = await admin.req('GET', `/commissions/export.csv?month=${thisMonth}`, undefined, true) as Response;
    const text = await csv.text();
    ok('a payroll export', csv.status === 200 && text.startsWith('month,employee,email,earned,taken_back,net,currency') && text.includes(`sam.${s}@acme.test`) && text.includes('180'), text.slice(0, 300));
    ok('the future can\'t be locked', (await admin.post('/commissions/months/2999-01/lock', {})).status === 400);
    r = await admin.post(`/commissions/months/${thisMonth}/lock`, {});
    made.months.push(thisMonth);
    ok('a month locked for payroll', r.status === 200 && r.data.locked === true);
    await pay(i1.id, '250.00');
    const next = (() => { const d = new Date(today.slice(0, 7) + '-01T00:00:00Z'); d.setUTCMonth(d.getUTCMonth() + 1); return d.toISOString().slice(0, 7); })();
    made.months.push(next);
    ok('later changes land in the next open month', n((await stmt()).net) === 180 && n((await stmt(next)).net) === 30, [(await stmt()).net, (await stmt(next)).net]);
    ok('recalculating changes nothing that is already right', (await admin.post('/commissions/recalculate', {})).data.new_entries === 0);
  }
} finally {
  await db.query('DELETE FROM comm_entries WHERE plan_id = ANY($1)', [made.plans]).catch(() => {});
  await db.query('DELETE FROM comm_plans WHERE id = ANY($1)', [made.plans]).catch(() => {});
  await db.query("DELETE FROM comm_months WHERE to_char(month, 'YYYY-MM') = ANY($1)", [made.months]).catch(() => {});
  const entries = (await db.query(
    `SELECT id FROM acc_entries WHERE source_id = ANY($1) OR source_id = ANY($2)`, [made.items, made.payments])).rows.map((r) => r.id);
  await db.query('DELETE FROM acc_credit_notes WHERE invoice_id = ANY($1)', [made.items]).catch((e) => console.log('cleanup:', e.message));
  await db.query('DELETE FROM acc_allocations WHERE payment_id = ANY($1)', [made.payments]).catch((e) => console.log('cleanup:', e.message));
  await db.query('DELETE FROM acc_payments WHERE id = ANY($1)', [made.payments]).catch((e) => console.log('cleanup:', e.message));
  await db.query('DELETE FROM acc_invoices WHERE item_id = ANY($1)', [made.items]).catch((e) => console.log('cleanup:', e.message));
  if (entries.length) {
    await db.query('DELETE FROM acc_lines WHERE entry_id = ANY($1)', [entries]).catch((e) => console.log('cleanup:', e.message));
    await db.query('UPDATE acc_entries SET reverses = NULL, reversed_by = NULL WHERE id = ANY($1)', [entries]).catch(() => {});
    await db.query('DELETE FROM acc_entries WHERE id = ANY($1)', [entries]).catch((e) => console.log('cleanup:', e.message));
  }
  await db.query('DELETE FROM crm_items WHERE id = ANY($1) OR company_name = $2', [made.items, `Buyer ${s}`]).catch((e) => console.log('cleanup:', e.message));
  await db.query('DELETE FROM crm_products WHERE id = ANY($1)', [made.products]).catch(() => {});
  await db.end();
  await cleanupRun(s);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
