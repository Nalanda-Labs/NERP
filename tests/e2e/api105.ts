// Inventory costing: receipts at their price make cost layers and a moving average; goods
// leaving cost the average or, by FIFO, the oldest layers first; the method can be switched
// midway; transfers keep their cost; a cancelled write-off comes back at the cost it left at;
// the stock's value and the cost of goods written off for a period; deals' gross profit uses
// the stock's cost; and, with posting on, each movement is posted to the books.
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
const before = (await admin.get('/inventory/settings')).data.settings;
const base = (await admin.get('/crm/config')).data.base_currency;
const doc = async (b: any) => {
  const d = (await admin.post('/inventory/documents', { comment: `cost ${s}`, ...b })).data;
  const r = await admin.post(`/inventory/documents/${d.id}/post`, {});
  return r.data?.id ? r.data : d;
};
const value = async (wh?: string) => (await admin.get(`/inventory/valuation${wh ? `?warehouse_id=${wh}` : ''}`)).data;
const item = (v: any, name: string) => v.items.find((x: any) => x.name === name);
const lastCost = async (product: string) => (await admin.get(`/inventory/products/${product}`)).data.movements[0];
let productId = '';

try {
  const north = (await admin.post('/inventory/warehouses', { name: `North ${s}` })).data.items.find((w: any) => w.name === `North ${s}`);
  const south = (await admin.post('/inventory/warehouses', { name: `South ${s}` })).data.items.find((w: any) => w.name === `South ${s}`);
  const p = (await admin.post('/crm/products', { name: `Widget ${s}`, price: '20', currency: base })).data;
  productId = p.id;
  ok('only stock managers set the method', (await (await (async () => { const c = new Client(); await admin.post('/users', { email: `nobody.${s}@acme.test`, password: 'password123', first_name: 'No', last_name: `Body${s}` }); await c.post('/auth/login', { email: `nobody.${s}@acme.test`, password: 'password123' }); return c; })()).put('/inventory/settings', { cost_method: 'fifo' })).status === 403);
  ok('a method is fifo or average', (await admin.put('/inventory/settings', { cost_method: 'lifo' })).status === 400);
  await admin.put('/inventory/settings', { cost_method: 'average', post_to_books: false });

  // ---- in at their price: 10 × 5, then 10 × 8 ----
  await doc({ kind: 'receipt', to_warehouse: north.id, items: [{ product_id: p.id, quantity: 10, price: 5 }] });
  let m = await lastCost(p.id);
  ok('a receipt brings goods in at their price', Number(m.unit_cost) === 5 && Number(m.cost) === 50, m);
  await doc({ kind: 'receipt', to_warehouse: north.id, items: [{ product_id: p.id, quantity: 10, price: 8 }] });
  let v = await value(north.id);
  ok('average: 20 on hand at 6.50, worth 130', Number(item(v, `Widget ${s}`)?.unit_cost) === 6.5 && Number(item(v, `Widget ${s}`)?.value) === 130 && v.method === 'average', v);

  // ---- out by average ----
  await doc({ kind: 'writeoff', from_warehouse: north.id, items: [{ product_id: p.id, quantity: 4 }] });
  m = await lastCost(p.id);
  ok('writing off 4 costs the average (26)', Number(m.cost) === -26 && Number(m.unit_cost) === 6.5, m);

  // ---- switch to FIFO: the oldest layers first ----
  ok('switching to FIFO', (await admin.put('/inventory/settings', { cost_method: 'fifo' })).data.settings.cost_method === 'fifo');
  v = await value(north.id);
  // The first 4 went from the oldest layer: 6 left at 5, 10 at 8.
  ok('FIFO value: the layers left (6 × 5 + 10 × 8 = 110)', Number(item(v, `Widget ${s}`)?.value) === 110, item(v, `Widget ${s}`));
  const wo = await doc({ kind: 'writeoff', from_warehouse: north.id, items: [{ product_id: p.id, quantity: 8 }] });
  m = await lastCost(p.id);
  ok('writing off 8 by FIFO: 6 × 5 + 2 × 8 = 46', Number(m.cost) === -46 && Number(m.unit_cost) === 5.75, m);

  // ---- a transfer keeps the cost ----
  await doc({ kind: 'transfer', from_warehouse: north.id, to_warehouse: south.id, items: [{ product_id: p.id, quantity: 3 }] });
  const sv = await value(south.id), nv = await value(north.id);
  ok('a transfer keeps its cost (3 × 8 in the other warehouse)', Number(item(sv, `Widget ${s}`)?.value) === 24 && Number(item(nv, `Widget ${s}`)?.value) === 40, [item(sv, `Widget ${s}`), item(nv, `Widget ${s}`)]);

  // ---- deals: gross profit at the stock's cost ----
  const deal = (await admin.post('/crm/deal/items', { title: `Costed deal ${s}` })).data;
  const rows = (await admin.put(`/crm/items/${deal.id}/products`, { rows: [{ product_id: p.id, name: `Widget ${s}`, price: '20', quantity: '2', unit: 'pcs', discount_kind: 'percent', discount_value: '0', tax_rate: null, tax_included: false }] })).data;
  ok('a deal row takes the stock\'s cost (FIFO: the next unit out, 8)', Number(rows.rows?.[0]?.row?.cost) === 8, rows.rows?.[0]?.row);
  await admin.del(`/crm/deal/items/${deal.id}`);

  // ---- cancelling the FIFO write-off: back at the cost it left at ----
  await admin.post(`/inventory/documents/${wo.id}/cancel`, {});
  m = await lastCost(p.id);
  ok('a cancelled write-off comes back at its cost (8 × 5.75)', Number(m.cost) === 46 && Number(m.unit_cost) === 5.75, m);

  const today = new Date().toISOString().slice(0, 10);
  const c = (await admin.get(`/inventory/cogs?from=${today}&to=${today}`)).data;
  const mine = c.items.filter((x: any) => x.name === `Widget ${s}`);
  ok('the period\'s write-offs: 26 (the cancelled one undone)', mine.length === 1 && mine[0].kind === 'written_off' && Number(mine[0].cost) === 26 && Number(mine[0].quantity) === 4, mine);

  // ---- posting to the books ----
  const acc = (await admin.get('/inventory/settings')).data;
  if (!acc.accounting_ready) {
    console.log('books part skipped: accounting is not set up');
  } else {
    ok('posting needs the accounts', (await admin.put('/inventory/settings', { post_to_books: true, inventory_account: null })).status === 400 || !!before.inventory_account);
    const accounts = (await admin.get('/accounting/accounts')).data.items;
    const by = (code: string) => accounts.find((a: any) => a.code === code)?.id;
    let r = await admin.put('/inventory/settings', { post_to_books: true, inventory_account: by('1200'), cogs_account: by('5000'), receipt_account: by('2000'), adjust_account: by('6990') });
    ok('posting turned on', r.status === 200 && r.data.settings.post_to_books, r.data);
    await doc({ kind: 'receipt', to_warehouse: north.id, items: [{ product_id: p.id, quantity: 2, price: 9.5 }] });
    m = await lastCost(p.id);
    const lines = (await db.query('SELECT a.code, l.debit, l.credit FROM acc_lines l JOIN acc_accounts a ON a.id = l.account_id WHERE l.entry_id = $1 ORDER BY a.code', [m.entry_id])).rows;
    ok('a receipt is posted: inventory up, goods received owed (19)', !!m.entry_id && lines.length === 2 && lines.some((l: any) => l.code === '1200' && Number(l.debit) === 19) && lines.some((l: any) => l.code === '2000' && Number(l.credit) === 19), lines);
    await doc({ kind: 'writeoff', from_warehouse: north.id, items: [{ product_id: p.id, quantity: 1 }] });
    m = await lastCost(p.id);
    const wl = (await db.query('SELECT a.code, l.debit, l.credit FROM acc_lines l JOIN acc_accounts a ON a.id = l.account_id WHERE l.entry_id = $1 ORDER BY a.code', [m.entry_id])).rows;
    ok('a write-off is posted: stock adjustment against inventory', wl.some((l: any) => l.code === '6990' && Number(l.debit) === Math.abs(Number(m.cost))) && wl.some((l: any) => l.code === '1200' && Number(l.credit) === Math.abs(Number(m.cost))), [m.cost, wl]);
    await doc({ kind: 'transfer', from_warehouse: north.id, to_warehouse: south.id, items: [{ product_id: p.id, quantity: 1 }] });
    ok('transfers aren\'t posted', !(await lastCost(p.id)).entry_id);
  }
} finally {
  await admin.put('/inventory/settings', { cost_method: before.cost_method, post_to_books: false }).catch(() => {});
  if (before.post_to_books) await admin.put('/inventory/settings', before).catch(() => {});
  // The test's entries leave the books (the app never deletes postings).
  if (productId) {
    const entries = (await db.query("SELECT entry_id FROM inv_movements WHERE product_id = $1 AND entry_id IS NOT NULL", [productId])).rows.map((r) => r.entry_id);
    if (entries.length) {
      await db.query('DELETE FROM acc_lines WHERE entry_id = ANY($1)', [entries]).catch((e) => console.log('cleanup:', e.message));
      await db.query('DELETE FROM acc_entries WHERE id = ANY($1)', [entries]).catch((e) => console.log('cleanup:', e.message));
    }
  }
  await db.end();
  await cleanupRun(s);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
