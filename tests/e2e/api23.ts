// Inventory: permissions, warehouses (default, deactivating, deleting), stock from the
// product form, receipts, write-offs, transfers and stock counts (posting all or nothing,
// cancelling), movement history, the stock list and its filters, and the shop selling from
// a chosen warehouse.
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 700)); };
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const sam = new Client();
await admin.post('/users', { email: `sam.${stamp}@acme.test`, password: 'password123', first_name: 'Sam', last_name: 'Store' });
await sam.post('/auth/login', { email: `sam.${stamp}@acme.test`, password: 'password123' });
const pub = new Client();

const whs = async () => (await admin.get('/inventory/warehouses')).data.items;
const originalDefault = (await whs()).find((w: any) => w.is_default);
const shopBefore = (await admin.get('/shop/settings')).data.settings;
const base = (await admin.get('/crm/config')).data.base_currency;
const per = async (product: string) => {
  const r = (await admin.get(`/inventory/products/${product}`)).data;
  return { total: r.total === null ? null : Number(r.total), ...Object.fromEntries(r.warehouses.map((w: any) => [w.name, Number(w.quantity)])) } as Record<string, number | null>;
};
const doc = (b: any) => admin.post('/inventory/documents', { comment: `test ${stamp}`, ...b });

try {
  // ---- permissions ----
  ok('warehouses need the permission', (await sam.post('/inventory/warehouses', { name: `X ${stamp}` })).status === 403);
  ok('documents need the permission', (await sam.post('/inventory/documents', { kind: 'receipt' })).status === 403);
  ok('anyone can see stock', (await sam.get('/inventory/stock')).status === 200);

  // ---- warehouses ----
  let r = await admin.post('/inventory/warehouses', { name: `North ${stamp}`, address: '1 Cold St' });
  const north = r.data.items.find((w: any) => w.name === `North ${stamp}`);
  const south = (await admin.post('/inventory/warehouses', { name: `South ${stamp}` })).data.items.find((w: any) => w.name === `South ${stamp}`);
  const spare = (await admin.post('/inventory/warehouses', { name: `Spare ${stamp}` })).data.items.find((w: any) => w.name === `Spare ${stamp}`);
  ok('warehouses made', north && south && spare && !north.is_default && north.address === '1 Cold St');
  ok('the default cannot be deactivated', (await admin.patch(`/inventory/warehouses/${originalDefault.id}`, { is_active: false })).status === 400);

  // ---- products: stock from the product form ----
  const mk = async (name: string, stock?: string) => (await admin.post('/crm/products', { name, price: '10', currency: base, ...(stock !== undefined ? { stock } : {}) })).data;
  const a = await mk(`Anvil ${stamp}`, '10');
  const b = await mk(`Bolt ${stamp}`);
  ok('form stock lands in the default warehouse', (await per(a.id))[originalDefault.name] === 10 && Number(a.stock) === 10);
  ok('uncounted product', b.stock === null);
  r = await admin.patch(`/crm/products/${a.id}`, { name: a.name, price: '10', currency: base, stock: '7' });
  ok('lowering the total takes from the default', r.status === 200 && (await per(a.id))[originalDefault.name] === 7, r.data);
  r = await admin.patch(`/crm/products/${a.id}`, { name: a.name, price: '10', currency: base });
  ok('saving without stock leaves it', Number(r.data.stock) === 7);

  // ---- receipt ----
  r = await doc({ kind: 'receipt', to_warehouse: north.id, reference: `INV-${stamp}`, items: [{ product_id: a.id, quantity: 5, price: 3.5 }, { product_id: b.id, quantity: 4, price: 1 }] });
  const rc = r.data;
  ok('receipt draft', r.status === 200 && rc.number.startsWith('RC-') && rc.status === 'draft' && rc.items.length === 2 && Number(rc.cost) === 21.5 && rc.currency === base, r.data);
  ok('drafts change nothing', (await per(a.id)).total === 7);
  ok('zero quantities rejected', (await admin.patch(`/inventory/documents/${rc.id}`, { items: [{ product_id: a.id, quantity: 0 }] })).status === 400);
  ok('suppliers are companies', (await admin.patch(`/inventory/documents/${rc.id}`, { supplier_id: a.id })).status === 400);
  const supplier = (await admin.post('/crm/company/items', { company_name: `Supplier ${stamp}` })).data;
  ok('supplier set', (await admin.patch(`/inventory/documents/${rc.id}`, { supplier_id: supplier.id })).data.supplier_name === `Supplier ${stamp}`);
  r = await admin.post(`/inventory/documents/${rc.id}/post`, {});
  ok('receipt posted', r.status === 200 && r.data.status === 'posted' && r.data.posted_at, r.data);
  let pa = await per(a.id), pb = await per(b.id);
  ok('stock arrived', pa[`North ${stamp}`] === 5 && pa.total === 12 && pb[`North ${stamp}`] === 4 && pb.total === 4, [pa, pb]);
  ok('posted documents are fixed', (await admin.patch(`/inventory/documents/${rc.id}`, { comment: 'x' })).status === 400 && (await admin.req('DELETE', `/inventory/documents/${rc.id}`)).status === 400);
  ok('posting twice refused', (await admin.post(`/inventory/documents/${rc.id}/post`, {})).status === 400);

  // ---- write-off: all or nothing ----
  const wo = (await doc({ kind: 'writeoff', from_warehouse: north.id, items: [{ product_id: b.id, quantity: 1 }, { product_id: a.id, quantity: 6 }] })).data;
  r = await admin.post(`/inventory/documents/${wo.id}/post`, {});
  ok('not more than the warehouse holds', r.status === 400 && /only 5 of “Anvil/.test(JSON.stringify(r.data)), r.data);
  ok('nothing changed', (await per(b.id))[`North ${stamp}`] === 4 && (await admin.get(`/inventory/documents/${wo.id}`)).data.status === 'draft');
  await admin.patch(`/inventory/documents/${wo.id}`, { items: [{ product_id: b.id, quantity: 1 }, { product_id: a.id, quantity: 2 }] });
  ok('write-off posted', (await admin.post(`/inventory/documents/${wo.id}/post`, {})).data.status === 'posted');
  pa = await per(a.id);
  ok('written off', pa[`North ${stamp}`] === 3 && (await per(b.id))[`North ${stamp}`] === 3);

  // ---- transfer ----
  const tr = (await doc({ kind: 'transfer', from_warehouse: north.id, to_warehouse: north.id, items: [{ product_id: a.id, quantity: 3 }] })).data;
  ok('transfer to the same warehouse refused', (await admin.post(`/inventory/documents/${tr.id}/post`, {})).status === 400);
  await admin.patch(`/inventory/documents/${tr.id}`, { to_warehouse: south.id });
  await admin.post(`/inventory/documents/${tr.id}/post`, {});
  pa = await per(a.id);
  ok('transferred', pa[`North ${stamp}`] === 0 && pa[`South ${stamp}`] === 3 && pa.total === 10, pa);

  // ---- stock count ----
  ok('a count lists each product once', (await doc({ kind: 'count', to_warehouse: south.id, items: [{ product_id: a.id, quantity: 1 }, { product_id: a.id, quantity: 2 }] })).status === 400);
  const extra = await mk(`Clamp ${stamp}`);
  const ic = (await doc({ kind: 'count', to_warehouse: south.id, items: [{ product_id: a.id, quantity: 1 }, { product_id: b.id, quantity: 7 }, { product_id: extra.id, quantity: 0 }] })).data;
  r = await admin.post(`/inventory/documents/${ic.id}/post`, {});
  const applied = Object.fromEntries(r.data.items.map((l: any) => [l.name, Number(l.applied)]));
  ok('count sets what was found', r.data.status === 'posted' && applied[`Anvil ${stamp}`] === -2 && applied[`Bolt ${stamp}`] === 7 && applied[`Clamp ${stamp}`] === 0, r.data.items);
  pa = await per(a.id); pb = await per(b.id);
  ok('counted', pa[`South ${stamp}`] === 1 && pb[`South ${stamp}`] === 7 && pb.total === 10 && (await per(extra.id)).total === 0, [pa, pb]);

  // ---- cancelling ----
  r = await admin.post(`/inventory/documents/${rc.id}/cancel`, {});
  ok('a receipt whose goods are gone cannot be cancelled', r.status === 400 && /North/.test(JSON.stringify(r.data)), r.data);
  r = await admin.post(`/inventory/documents/${ic.id}/cancel`, {});
  ok('cancelling a count undoes it', r.data.status === 'cancelled' && (await per(a.id))[`South ${stamp}`] === 3 && (await per(b.id))[`South ${stamp}`] === 0);
  await admin.post(`/inventory/documents/${wo.id}/cancel`, {});
  ok('cancelling a write-off brings goods back', (await per(a.id))[`North ${stamp}`] === 2 && (await per(b.id))[`North ${stamp}`] === 4);
  ok('cancelled documents cannot be posted', (await admin.post(`/inventory/documents/${wo.id}/post`, {})).status === 400);
  const draftDoc = (await doc({ kind: 'writeoff' })).data;
  ok('drafts start in the default warehouse and can be deleted', draftDoc.from_warehouse === originalDefault.id && (await admin.req('DELETE', `/inventory/documents/${draftDoc.id}`)).status === 200);

  // ---- history and lists ----
  const h = (await admin.get(`/inventory/products/${a.id}`)).data;
  const reasons = h.movements.map((m: any) => m.reason);
  ok('movement history', reasons.includes('product_form') && reasons.includes('document') && reasons.includes('document_cancelled') && h.movements.some((m: any) => m.document_number === tr.number) && h.movements[0].balance !== undefined, reasons);
  r = await admin.get(`/inventory/stock?q=${stamp}`);
  ok('stock list', r.data.total === 3 && Number(r.data.items.find((i: any) => i.name === `Anvil ${stamp}`).per[south.id]) === 3, r.data.items);
  ok('out of stock filter', (await admin.get(`/inventory/stock?q=${stamp}&state=out`)).data.items.map((i: any) => i.name).join() === `Clamp ${stamp}`);
  ok('low stock in one warehouse', (await admin.get(`/inventory/stock?q=${stamp}&state=low&warehouse=${north.id}`)).data.items.map((i: any) => i.name).sort().join() === [`Anvil ${stamp}`, `Bolt ${stamp}`].sort().join());
  r = await admin.get(`/inventory/documents?q=${stamp}`);
  ok('documents list', r.data.total >= 1 && r.data.items.some((d: any) => d.number === rc.number && d.supplier_name === `Supplier ${stamp}` && Number(d.cost) === 21.5), r.data);

  // ---- warehouses in use ----
  ok('a warehouse holding stock cannot be deactivated', (await admin.patch(`/inventory/warehouses/${north.id}`, { is_active: false })).status === 400);
  ok('a used warehouse cannot be deleted', (await admin.req('DELETE', `/inventory/warehouses/${north.id}`)).status === 400);
  ok('an unused one can', (await admin.req('DELETE', `/inventory/warehouses/${spare.id}`)).data.items?.every((w: any) => w.id !== spare.id));
  r = await admin.patch(`/inventory/warehouses/${south.id}`, { is_default: true });
  ok('new default', r.data.items.filter((w: any) => w.is_default).map((w: any) => w.id).join() === south.id);
  ok('drafts follow the default', (await doc({ kind: 'receipt' })).data.to_warehouse === south.id);
  await admin.patch(`/inventory/warehouses/${originalDefault.id}`, { is_default: true });
  ok('cannot stop counting stock that warehouses hold', (await admin.patch(`/crm/products/${a.id}`, { name: a.name, price: '10', currency: base, stock: null })).status === 400);
  ok('products on documents are deactivated, not deleted', (await admin.req('DELETE', `/crm/products/${b.id}`)).data.deactivated === true);

  // ---- the shop sells from its warehouse ----
  await admin.patch(`/crm/products/${a.id}`, { name: a.name, price: '10', currency: base, in_shop: true });
  r = await admin.patch('/shop/settings', { warehouse_id: south.id, enabled: true, payment_instructions: 'Pay by transfer', terms: null });
  ok('shop warehouse set', r.data.settings.warehouse_id === south.id, r.data);
  const listed = (await pub.get(`/public/shop/products?q=${stamp}`)).data.items.find((p: any) => p.name === `Anvil ${stamp}`);
  ok('the shop shows its warehouse', Number(listed?.stock) === 3, listed);
  r = await pub.post('/public/shop/checkout', { name: `Ola ${stamp}`, email: `ola.${stamp}@example.com`, address: 'Here', items: [{ slug: listed.slug, quantity: 2 }] });
  pa = await per(a.id);
  ok('an order takes from that warehouse', r.status === 200 && pa[`South ${stamp}`] === 1 && pa[`North ${stamp}`] === 2, [r.data, pa]);
  const om = (await admin.get(`/inventory/products/${a.id}`)).data.movements[0];
  ok('the movement links the order', om.reason === 'order' && om.item_code === 'order' && om.item_number === r.data.number, om);
} finally {
  await admin.patch(`/inventory/warehouses/${originalDefault.id}`, { is_default: true });
  await admin.patch('/shop/settings', shopBefore);
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
