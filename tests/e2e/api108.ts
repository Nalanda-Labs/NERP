// Cash forecast: open invoices expected on their due date plus how late the client usually
// pays; CRM invoices not in the books yet the same way; open deals × their chance, on the close
// date plus how long the client takes to pay, less what is already invoiced; undated deals
// apart; open bills going out; the bank balance it leads to; only for the books' readers.
//
// Acme's history: an invoice dated 40 days ago, due 30 days ago, paid 20 days ago — 10 days
// late, 20 days from invoice to payment.
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
const day = (offset: number) => new Date(Date.now() + offset * 86400e3).toISOString().slice(0, 10);
const n = (v: any) => Number(v);
const made: { items: string[]; bills: string[]; payments: string[] } = { items: [], bills: [], payments: [] };

try {
  const ready = (await admin.get('/inventory/settings')).data.accounting_ready;
  const sam = new Client();
  await admin.post('/users', { email: `sam.${s}@acme.test`, password: 'password123', first_name: 'Sam', last_name: `Cash${s}` });
  await sam.post('/auth/login', { email: `sam.${s}@acme.test`, password: 'password123' });
  ok('only for the books\' readers', (await sam.get('/accounting/cash-forecast')).status === 403);
  ok('weeks are checked', (await admin.get('/accounting/cash-forecast?weeks=99')).status === 400);
  // Whatever the books hold (no paid invoices yet included).
  ok('a forecast at any time', (await admin.get('/accounting/cash-forecast')).status === 200);
  if (!ready) {
    console.log('skipped: accounting is not set up');
  } else {
    const accounts = (await admin.get('/accounting/accounts')).data.items;
    const bank = accounts.find((a: any) => a.subtype === 'bank' && a.is_active);
    const expense = accounts.find((a: any) => a.type === 'expense' && a.subtype === 'expense' && a.is_active);
    const acme = (await admin.post('/crm/company/items', { company_name: `Acme ${s}` })).data;
    const supplier = (await admin.post('/crm/company/items', { company_name: `Supplier ${s}` })).data;
    const invoice = async (title: string, amount: string, due: string) => {
      const i = (await admin.post('/crm/invoice/items', { title: `${title} ${s}`, company_id: acme.id, currency: base, close_date: due, opportunity: amount, is_manual_amount: true })).data;
      made.items.push(i.id);
      return i;
    };
    // History: paid 10 days late.
    const h = await invoice('History', '100', day(-30));
    await admin.post(`/accounting/invoices/${h.id}/post`, { date: day(-40) });
    let r = await admin.post('/accounting/payments', { direction: 'in', date: day(-20), account_id: bank.id, amount: '100.00', allocations: [{ doc_kind: 'invoice', doc_id: h.id, amount: '100.00' }] });
    made.payments.push(r.data.id);
    // Open, due in 7 days: expected 17 days from now.
    const i1 = await invoice('Open', '500', day(7));
    await admin.post(`/accounting/invoices/${i1.id}/post`, { date: day(0) });
    // Not in the books, due in 3 days: expected in 13.
    const i2 = await invoice('Draft', '200', day(3));
    // Deals: 40% of 1000, closing in 14 days (paid 20 days later: day 34); one without a date;
    // one half invoiced already (300 − 100 at 50%, closing in 7: day 27).
    const d1 = (await admin.post('/crm/deal/items', { title: `Expansion ${s}`, company_id: acme.id, currency: base, opportunity: '1000', is_manual_amount: true, probability: 40, close_date: day(14) })).data;
    const d2 = (await admin.post('/crm/deal/items', { title: `Someday ${s}`, company_id: acme.id, currency: base, opportunity: '800', is_manual_amount: true, probability: 25 })).data;
    const d3 = (await admin.post('/crm/deal/items', { title: `Partly ${s}`, company_id: acme.id, currency: base, opportunity: '300', is_manual_amount: true, probability: 50, close_date: day(7) })).data;
    made.items.push(d1.id, d2.id, d3.id);
    const part = await invoice('Part', '100', day(40));
    await db.query('UPDATE crm_items SET source_item_id = $2 WHERE id = $1', [part.id, d3.id]);
    // A bill due in 5 days.
    const b = (await admin.post('/accounting/bills', { party_id: supplier.id, reference: `BILL-${s}`, date: day(0), due_date: day(5), lines: [{ description: 'Rent', account_id: expense.id, quantity: '1', unit_price: '150.00' }] })).data;
    made.bills.push(b.id);
    await admin.post(`/accounting/bills/${b.id}/post`, {});

    r = await admin.get('/accounting/cash-forecast?weeks=13');
    ok('a forecast of 13 weeks', r.status === 200 && r.data.weeks.length === 13 && typeof r.data.opening === 'string', r.data);
    const all = r.data.weeks.flatMap((w: any) => w.items.map((x: any) => ({ ...x, week: w.start })));
    const find = (id: string) => all.find((x: any) => x.id === id);
    const weekOf = (date: string) => r.data.weeks.find((w: any) => w.start <= date && date < new Date(new Date(w.start).getTime() + 7 * 86400e3).toISOString().slice(0, 10))?.start;
    let x = find(i1.id);
    ok('an open invoice: due + the client\'s 10 days late', x?.date === day(17) && n(x.amount) === 500 && x.week === weekOf(day(17)), x);
    x = find(i2.id);
    ok('a CRM invoice not in the books: the same way', x?.date === day(13) && n(x.amount) === 200 && x.kind === 'crm_invoice', x);
    x = find(d1.id);
    ok('a deal: 40% of 1000, close + the client\'s 20 days to pay', x?.date === day(34) && n(x.amount) === 400 && n(x.full) === 1000 && n(x.chance) === 40, x);
    x = find(d3.id);
    ok('a partly invoiced deal: what is left (200) at 50%', x?.date === day(27) && n(x.amount) === 100 && n(x.full) === 200, x);
    ok('a deal without a close date is listed apart', r.data.undated_deals.some((u: any) => u.id === d2.id && n(u.amount) === 800 && n(u.chance) === 25) && !find(d2.id));
    x = find(b.id);
    ok('a bill going out on its due date', x?.dir === 'out' && x.date === day(5) && n(x.amount) === 150, x);
    ok('a paid invoice isn\'t expected again', !find(h.id));
    const w0 = r.data.weeks[0];
    ok('the balance runs on from the bank: likely, at best, invoices only', n(r.data.weeks.at(-1).best) >= n(r.data.weeks.at(-1).balance) && n(r.data.weeks.at(-1).balance) >= n(r.data.weeks.at(-1).worst)
      && Math.abs(n(w0.balance) - (n(r.data.opening) + n(w0.net))) < 0.01, [r.data.opening, w0]);
  }
} finally {
  // The test's documents leave the books (the app never deletes postings).
  const entries = (await db.query(
    `SELECT entry_id FROM acc_invoices WHERE item_id = ANY($1) UNION SELECT entry_id FROM acc_bills WHERE id = ANY($2) AND entry_id IS NOT NULL
     UNION SELECT entry_id FROM acc_payments WHERE id = ANY($3) AND entry_id IS NOT NULL`, [made.items, made.bills, made.payments])).rows.map((r) => r.entry_id);
  await db.query('DELETE FROM acc_allocations WHERE payment_id = ANY($1)', [made.payments]).catch((e) => console.log('cleanup:', e.message));
  await db.query('DELETE FROM acc_payments WHERE id = ANY($1)', [made.payments]).catch((e) => console.log('cleanup:', e.message));
  await db.query('DELETE FROM acc_invoices WHERE item_id = ANY($1)', [made.items]).catch((e) => console.log('cleanup:', e.message));
  await db.query('DELETE FROM acc_bill_lines WHERE bill_id = ANY($1)', [made.bills]).catch((e) => console.log('cleanup:', e.message));
  await db.query('DELETE FROM acc_bills WHERE id = ANY($1)', [made.bills]).catch((e) => console.log('cleanup:', e.message));
  if (entries.length) {
    await db.query('DELETE FROM acc_lines WHERE entry_id = ANY($1)', [entries]).catch((e) => console.log('cleanup:', e.message));
    await db.query('DELETE FROM acc_entries WHERE id = ANY($1)', [entries]).catch((e) => console.log('cleanup:', e.message));
  }
  await db.end();
  await cleanupRun(s);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
