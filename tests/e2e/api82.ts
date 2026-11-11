// Gross profit and margin: a product's cost price (or, without one, its average purchase price
// from posted receipts) travels onto a deal's product rows when they're saved, and the profit
// report adds up net sales, costs, profit and margin of the deals won, by manager, product,
// month and deal, only over deals the person may read.
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const mk = async (first: string) => {
  const email = `${first.toLowerCase()}.${stamp}@acme.test`;
  const u = (await admin.post('/users', { email, password: 'password123', first_name: first, last_name: `Gp${stamp}` })).data;
  const c = new Client();
  await c.post('/auth/login', { email, password: 'password123' });
  c.id = u.id;
  return c;
};
const products: string[] = [];
let catId = '';
let wh = '';

try {
  const ann = await mk('Ann'), bob = await mk('Bob'), cat = await mk('Cat');
  const base = (await admin.get('/crm/config')).data.base_currency;
  const product = async (b: any) => { const r = await admin.post('/crm/products', { currency: base, ...b }); if (r.data?.id) products.push(r.data.id); return r; };

  let r = await product({ name: `Widget ${stamp}`, price: '100', cost: '60' });
  ok('a product with a cost price', r.status === 200 && Number(r.data.cost) === 60, r.data);
  const A = r.data;
  ok('negative costs are refused', (await product({ name: `Bad ${stamp}`, price: '1', cost: '-1' })).status === 400);
  const B = (await product({ name: `Gadget ${stamp}`, price: '50' })).data;
  const C = (await product({ name: `Gizmo ${stamp}`, price: '80' })).data;
  ok('cost is optional', B.cost === null);

  // Gadget has no cost price but was bought twice: 10 at 20 and 10 at 30.
  wh = (await admin.post('/inventory/warehouses', { name: `Gp ${stamp}` })).data.items.find((w: any) => w.name === `Gp ${stamp}`).id;
  for (const price of [20, 30]) {
    const d = (await admin.post('/inventory/documents', { kind: 'receipt', to_warehouse: wh, comment: `gp ${stamp}`, items: [{ product_id: B.id, quantity: 10, price }] })).data;
    await admin.post(`/inventory/documents/${d.id}/post`, {});
  }

  r = await admin.post('/crm/deal/categories', { name: `Profit ${stamp}` });
  const pipe = r.data.categories.find((c: any) => c.name === `Profit ${stamp}`);
  catId = pipe.id;
  const won = pipe.stages.find((s: any) => s.semantics === 'success').id;
  const deal = async (who: Client, rows: any[] | null) => {
    const d = (await admin.post('/crm/deal/items', { title: `Deal ${stamp} ${who.id.slice(0, 4)}`, category_id: pipe.id, assigned_to: who.id, currency: base })).data;
    if (rows) {
      const x = await admin.put(`/crm/items/${d.id}/products`, { rows });
      if (x.status !== 200) throw new Error(JSON.stringify(x.data));
    }
    await admin.patch(`/crm/deal/items/${d.id}`, { stage_id: won });
    return d.id as string;
  };
  const row = (p: any, quantity: number, extra: any = {}) => ({ product_id: p.id, name: p.name, price: p.price, quantity: String(quantity), ...extra });
  const d1 = await deal(ann, [row(A, 2), row(B, 1)]);
  const d2 = await deal(bob, [row(C, 1), { product_id: null, name: `Install ${stamp}`, price: '40', quantity: '1', cost: '10' }]);
  await deal(bob, null);

  const rows1 = (await admin.get(`/crm/items/${d1}/products`)).data.rows.map((x: any) => x.row);
  ok('a catalogue row takes the product\'s cost price', Number(rows1[0].cost) === 60, rows1);
  ok('…or its average purchase price', Number(rows1[1].cost) === 25, rows1);
  const rows2 = (await admin.get(`/crm/items/${d2}/products`)).data.rows.map((x: any) => x.row);
  ok('unknown costs stay unknown', rows2[0].cost === null && Number(rows2[1].cost) === 10, rows2);

  await admin.patch(`/crm/products/${A.id}`, { cost: '90' });
  ok('changing the product later leaves past deals alone', Number((await admin.get(`/crm/items/${d1}/products`)).data.rows[0].row.cost) === 60);

  const q = `?category_id=${pipe.id}`;
  r = await admin.get(`/crm/analytics/deal/profit${q}`);
  const t = r.data.total;
  ok('the report: net sales and cost of the costed rows', r.status === 200 && Number(t.revenue) === 290 && Number(t.cost) === 155, r.data);
  ok('…gross profit and margin', Number(t.profit) === 135 && t.margin === '46.6', t);
  ok('…sales without a cost, and deals won without products, apart', Number(t.uncosted_revenue) === 80 && r.data.without_products === 1, r.data);
  const m = Object.fromEntries(r.data.by_manager.map((x: any) => [x.user_id, x]));
  ok('by manager', Number(m[ann.id]?.profit) === 105 && m[ann.id]?.margin === '42.0' && Number(m[bob.id]?.profit) === 30, r.data.by_manager);
  const w = r.data.by_product.find((x: any) => x.product_id === A.id);
  ok('by product', Number(w?.revenue) === 200 && Number(w?.cost) === 120 && Number(w?.quantity) === 2, r.data.by_product);
  ok('by month', r.data.by_month.length === 1 && Number(r.data.by_month[0].profit) === 135, r.data.by_month);
  ok('the deals, most profitable first', r.data.deals[0]?.id === d1 && r.data.deals.length === 2, r.data.deals);
  ok('a past period has nothing', Number((await admin.get(`/crm/analytics/deal/profit${q}&from=2020-01-01&to=2020-01-31`)).data.total.deals) === 0);
  ok('periods must make sense', (await admin.get(`/crm/analytics/deal/profit${q}&from=2026-02-01&to=2026-01-01`)).status === 400);

  // With employees reading only their own deals, Cat (who has none) sees no profit, and Ann
  // only hers.
  const emp = (await admin.get('/roles')).data.find((x: any) => x.code === 'employee');
  const perms = (await admin.get('/crm/permissions')).data;
  const original = Object.entries(perms.matrix[emp.id]).flatMap(([t, acts]: [string, any]) => Object.entries(acts).map(([action, scope]) => ({ entity_type: +t, action, scope })));
  try {
    await admin.put(`/crm/permissions/${emp.id}`, { entries: original.map((e) => (e.entity_type === 2 && e.action === 'read' ? { ...e, scope: 'own' } : e)) });
    ok('only deals the person may read count', (await cat.get(`/crm/analytics/deal/profit${q}`)).data.total.deals === 0);
    const hers = (await ann.get(`/crm/analytics/deal/profit${q}`)).data;
    ok('…so Ann sees her own', hers.total.deals === 1 && Number(hers.total.profit) === 105, hers.total);
  } finally {
    await admin.put(`/crm/permissions/${emp.id}`, { entries: original });
  }
} finally {
  if (catId) for (const d of (await admin.get(`/crm/deal/items?category_id=${catId}&limit=50`)).data.items ?? []) await admin.del(`/crm/deal/items/${d.id}`);
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
