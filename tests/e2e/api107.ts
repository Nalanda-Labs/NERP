// True profit: an invoice's revenue without tax after discounts, the discounts, what the goods
// cost, support time on the client's tickets and task time on its records at each person's
// hourly cost, and late payment at the financing rate — by client, deal, product and
// salesperson; hourly costs and settings; only for the books' readers and CRM admins.
//
// The client (Acme) bought 2 × Widget at 100 with 10% off (no tax): revenue 180, discounts 20,
// goods 2 × 40 = 80. Support spent 30 minutes at 60/h = 30; a task linked to Acme took an hour
// at 20/h = 20. Due 20 days before it was paid: 180 × 20/365 × 36.5% = 3.60.
// Profit: 180 − 80 − 30 − 20 − 3.60 = 46.40.
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { dbUrl } from '../db/cleanup.ts';
import pg from 'pg';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const s = Date.now().toString(36);
const admin = new Client();
const me = (await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' })).data.user;
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
const before = (await admin.get('/crm/profit/settings')).data;
const base = (await admin.get('/crm/config')).data.base_currency;
const day = (offset: number) => new Date(Date.now() + offset * 86400e3).toISOString().slice(0, 10);
const n = (v: any) => Number(v);
const entries: string[] = [];
let samId = '';

try {
  const samEmail = `sam.${s}@acme.test`;
  const samUser = (await admin.post('/users', { email: samEmail, password: 'password123', first_name: 'Sam', last_name: `Support${s}` })).data;
  samId = samUser.id;
  const sam = new Client();
  await sam.post('/auth/login', { email: samEmail, password: 'password123' });
  ok('only for the books\' readers and CRM administrators', (await sam.get('/crm/profit')).status === 403 && (await sam.put('/crm/profit/settings', { default_hourly: 1 })).status === 403);

  // ---- settings: hourly costs, financing ----
  ok('costs and rates are checked', (await admin.put('/crm/profit/settings', { financing_rate: 150 })).status === 400 && (await admin.put('/crm/profit/costs', [{ user_id: samId, hourly: -1 }])).status === 400);
  await admin.put('/crm/profit/settings', { default_hourly: 0, financing_rate: 36.5 });
  let r = await admin.put('/crm/profit/costs', [{ user_id: samId, hourly: 60 }, { user_id: me.id, hourly: 20 }]);
  ok('an hourly cost per person', r.status === 200 && n(r.data.people.find((p: any) => p.id === samId)?.hourly) === 60, r.data.people?.find((p: any) => p.id === samId));

  // ---- the client, its deal, its invoice ----
  const acme = (await admin.post('/crm/company/items', { company_name: `Acme ${s}` })).data;
  const kim = (await admin.post('/crm/contact/items', { first_name: 'Kim', last_name: `Profit${s}`, company_id: acme.id })).data;
  const deal = (await admin.post('/crm/deal/items', { title: `Big order ${s}`, company_id: acme.id, contact_ids: [kim.id] })).data;
  const widget = (await admin.post('/crm/products', { name: `Widget ${s}`, price: '100', currency: base, cost: '40' })).data;
  const inv = (await admin.post('/crm/invoice/items', { title: `Invoice ${s}`, company_id: acme.id, contact_ids: [kim.id], currency: base, close_date: day(-20) })).data;
  await db.query('UPDATE crm_items SET source_item_id = $2 WHERE id = $1', [inv.id, deal.id]);
  r = await admin.put(`/crm/items/${inv.id}/products`, { rows: [{ product_id: widget.id, name: `Widget ${s}`, price: '100', quantity: '2', unit: 'pcs', discount_kind: 'percent', discount_value: '10', tax_rate: null, tax_included: false }] });
  ok('the invoice: 2 × 100 with 10% off', r.status === 200 && n(r.data.totals.total) === 180, r.data.totals);

  // ---- time spent on the client ----
  const ticket = (await db.query("INSERT INTO support_tickets (title, channel, contact_id, responsible_id) VALUES ($1, 'staff', $2, $3) RETURNING id", [`Help ${s}`, kim.id, samId])).rows[0].id;
  await db.query("INSERT INTO support_messages (ticket_id, kind, by_support, author_id, body, minutes_spent) VALUES ($1, 'message', TRUE, $2, 'Answered', 30)", [ticket, samId]);
  const task = (await admin.post('/tasks', { title: `Prepare order ${s}`, responsible_id: me.id })).data;
  await db.query('INSERT INTO task_crm_links (task_id, item_id) VALUES ($1, $2) ON CONFLICT DO NOTHING', [task.id, acme.id]);
  await db.query("INSERT INTO task_time_entries (task_id, user_id, started_at, stopped_at) VALUES ($1, $2, now() - interval '2 hours', now() - interval '1 hour')", [task.id, me.id]);

  // ---- in the books: posted 30 days ago, due 20 days ago, paid today ----
  const acc = (await admin.get('/inventory/settings')).data.accounting_ready;
  if (acc) {
    r = await admin.post(`/accounting/invoices/${inv.id}/post`, { date: day(-30) });
    ok('posted to the books', r.status === 200, r.data);
    const bank = (await admin.get('/accounting/accounts')).data.items.find((a: any) => a.subtype === 'bank' && a.is_active);
    r = await admin.post('/accounting/payments', { direction: 'in', date: day(0), account_id: bank.id, amount: '180.00', allocations: [{ doc_kind: 'invoice', doc_id: inv.id, amount: '180.00' }] });
    ok('paid today', r.status === 200, r.data);
    entries.push(...(await db.query("SELECT entry_id FROM acc_invoices WHERE item_id = $1 UNION SELECT entry_id FROM acc_payments WHERE id = $2", [inv.id, r.data.id])).rows.map((x) => x.entry_id));
  }

  // ---- by client ----
  r = await admin.get(`/crm/profit?by=client&from=${day(-60)}&to=${day(0)}`);
  const row = r.data.items?.find((x: any) => x.id === acme.id);
  ok('by client: revenue, discounts, goods', r.status === 200 && n(row?.revenue) === 180 && n(row.discounts) === 20 && n(row.goods) === 80, row);
  ok('…support time (30 min at 60/h) and task time (1 h at 20/h)', n(row?.support) === 30 && n(row?.time) === 20, row);
  if (acc) {
    ok('…late payment (20 days at 36.5%) and the profit', n(row?.late) === 3.6 && n(row?.profit) === 46.4 && n(row?.margin) === 25.8, row);
  } else {
    ok('…and the profit (no books: nothing late)', n(row?.late) === 0 && n(row?.profit) === 50, row);
  }
  ok('…linked to the client', row?.link === `/crm/company/${acme.id}` && row.name === `Acme ${s}`, row);

  // ---- the same figures, other ways ----
  const expected = acc ? 46.4 : 50;
  r = await admin.get(`/crm/profit?by=deal&from=${day(-60)}&to=${day(0)}`);
  ok('by deal', n(r.data.items.find((x: any) => x.id === deal.id)?.profit) === expected, r.data.items.find((x: any) => x.id === deal.id));
  r = await admin.get(`/crm/profit?by=product&from=${day(-60)}&to=${day(0)}`);
  const p = r.data.items.find((x: any) => x.id === widget.id);
  ok('by product: its revenue and goods, the client\'s costs spread over it', n(p?.revenue) === 180 && n(p?.goods) === 80 && n(p?.profit) === expected, p);
  r = await admin.get(`/crm/profit?by=person&from=${day(-60)}&to=${day(0)}`);
  ok('by salesperson', r.data.items.some((x: any) => x.id === me.id && n(x.revenue) >= 180), r.data.items.map((x: any) => x.name));
  ok('another period leaves it out', !(await admin.get(`/crm/profit?by=client&from=${day(-400)}&to=${day(-200)}`)).data.items.some((x: any) => x.id === acme.id));
  ok('grouping is checked', (await admin.get('/crm/profit?by=planet')).status === 400);
} finally {
  await admin.put('/crm/profit/settings', { default_hourly: n(before.default_hourly), financing_rate: n(before.financing_rate) }).catch(() => {});
  await admin.put('/crm/profit/costs', [{ user_id: me.id, hourly: before.people?.find((x: any) => x.id === me.id)?.hourly ?? null }]).catch(() => {});
  await db.query('DELETE FROM support_tickets WHERE title = $1', [`Help ${s}`]).catch(() => {});
  if (entries.length) {
    await db.query("DELETE FROM acc_allocations WHERE doc_id IN (SELECT id FROM crm_items WHERE title = $1)", [`Invoice ${s}`]).catch((e) => console.log('cleanup:', e.message));
    await db.query("DELETE FROM acc_payments WHERE entry_id = ANY($1)", [entries]).catch((e) => console.log('cleanup:', e.message));
    await db.query("DELETE FROM acc_invoices WHERE item_id IN (SELECT id FROM crm_items WHERE title = $1)", [`Invoice ${s}`]).catch((e) => console.log('cleanup:', e.message));
    await db.query('DELETE FROM acc_lines WHERE entry_id = ANY($1)', [entries]).catch((e) => console.log('cleanup:', e.message));
    await db.query('DELETE FROM acc_entries WHERE id = ANY($1)', [entries]).catch((e) => console.log('cleanup:', e.message));
  }
  await db.end();
  await cleanupRun(s);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
