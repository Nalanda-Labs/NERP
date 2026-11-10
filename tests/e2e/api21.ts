// Commerce: shop settings and permissions, categories, shop fields of products (photo, stock),
// the public catalog, checkout (validation, stock, contacts, the order record, emails,
// notifications), the customer's order page, cancelling and reopening (stock back and
// taken again), invoices from orders, and the closed shop.
import { Client, waitFor } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 700)); };
const stamp = Date.now().toString(36);
const MP = 'http://localhost:8026/api/v1';
const mailpit = async (q: string): Promise<any[]> => (await (await fetch(`${MP}/search?query=${encodeURIComponent(q)}`)).json()).messages ?? [];
const pub = new Client(); // a shop visitor: no sign-in
const PNG = Uint8Array.from(atob('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=='), (c) => c.charCodeAt(0));

const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const sam = new Client();
const samEmail = `sam.${stamp}@acme.test`;
const samUser = (await admin.post('/users', { email: samEmail, password: 'password123', first_name: 'Sam', last_name: 'Shop' })).data;
await sam.post('/auth/login', { email: samEmail, password: 'password123' });
const before = (await admin.get('/shop/settings')).data.settings;

try {
  // ---- settings ----
  ok('shop settings need their permission', (await sam.get('/shop/settings')).status === 403 && (await sam.patch('/shop/settings', { enabled: true })).status === 403);
  await admin.patch('/shop/settings', { enabled: false });
  ok('closed shop is not found', (await pub.get('/public/shop')).status === 404 && (await pub.get('/public/shop/products')).status === 404);
  let r = await admin.patch('/shop/settings', { enabled: true, payment_instructions: null });
  ok('cannot open without payment instructions', r.status === 400 && /payment/.test(r.data.error?.message ?? JSON.stringify(r.data)), r.data);
  ok('unknown currency rejected', (await admin.patch('/shop/settings', { currency: 'XQQ' })).status === 400);
  ok('handlers must be active employees', (await admin.patch('/shop/settings', { handlers: ['00000000-0000-0000-0000-000000000123'] })).status === 400);
  const base = (await admin.get('/crm/config')).data.base_currency;
  r = await admin.patch('/shop/settings', {
    enabled: true, name: `Acme Store ${stamp}`, intro: 'Hello', currency: base, handlers: [samUser.id],
    payment_instructions: 'Transfer {{total}} with reference "Order {{order}}".', terms: 'No returns after 30 days.',
  });
  ok('shop opened', r.status === 200 && r.data.settings.enabled && r.data.handlers[0]?.name === 'Sam Shop' && r.data.shop_url.endsWith('/shop'), r.data);

  // ---- categories ----
  ok('categories need the catalog or shop permission', (await sam.post('/shop/categories', { name: `X ${stamp}` })).status === 403);
  r = await admin.post('/shop/categories', { name: `Mugs & Cups ${stamp}` });
  const mugs = r.data.items.find((c: any) => c.name === `Mugs & Cups ${stamp}`);
  ok('category made with an address', mugs?.slug === `mugs-cups-${stamp}`, r.data);
  const tees = (await admin.post('/shop/categories', { name: `Tees ${stamp}` })).data.items.find((c: any) => c.name === `Tees ${stamp}`);
  r = await admin.patch(`/shop/categories/${tees.id}`, { name: `T-shirts ${stamp}` });
  ok('renaming keeps the address', r.data.items.find((c: any) => c.id === tees.id)?.slug === tees.slug, r.data);

  // ---- products ----
  const fd = new FormData(); fd.append('file', new Blob([PNG], { type: 'image/png' }), `mug-${stamp}.png`);
  const photo = (await admin.req('POST', '/files', fd)).data;
  const txt = new FormData(); txt.append('file', new Blob(['hi'], { type: 'text/plain' }), `notes-${stamp}.txt`);
  const notPhoto = (await admin.req('POST', '/files', txt)).data;
  const samFd = new FormData(); samFd.append('file', new Blob([PNG], { type: 'image/png' }), `sams-${stamp}.png`);
  const samsPhoto = (await sam.req('POST', '/files', samFd)).data;
  const product = (b: any) => admin.post('/crm/products', { currency: base, ...b });
  ok('photo must be an image', (await product({ name: `Bad ${stamp}`, price: '1', in_shop: true, image_file_id: notPhoto.id })).status === 400);
  ok("photo must be one's own upload", (await product({ name: `Bad2 ${stamp}`, price: '1', in_shop: true, image_file_id: samsPhoto.id })).status === 400);
  ok('stock cannot be negative', (await product({ name: `Bad3 ${stamp}`, price: '1', stock: '-1' })).status === 400);
  ok('unknown category rejected', (await product({ name: `Bad4 ${stamp}`, price: '1', shop_category_id: '00000000-0000-0000-0000-000000000123' })).status === 400);
  r = await product({ name: `Mug ${stamp}`, description: 'A big mug', price: '10', tax_rate: '20', in_shop: true, shop_category_id: mugs.id, image_file_id: photo.id, stock: '3' });
  const mug = r.data;
  ok('shop product saved', r.status === 200 && mug.in_shop && mug.slug === `mug-${stamp}` && Number(mug.stock) === 3 && mug.image_file_id === photo.id, r.data);
  const tee = (await product({ name: `Tee ${stamp}`, price: '5', in_shop: true, shop_category_id: tees.id })).data;
  const hidden = (await product({ name: `Backroom ${stamp}`, price: '7' })).data;
  const old = (await product({ name: `Retired ${stamp}`, price: '7', in_shop: true })).data;
  await admin.patch(`/crm/products/${old.id}`, { name: old.name, price: '7', currency: base, is_active: false });
  r = await admin.patch(`/crm/products/${tee.id}`, { name: tee.name, price: '5', currency: base });
  ok('editing without shop fields keeps them', r.data.in_shop && r.data.shop_category_id === tees.id && r.data.slug === tee.slug, r.data);

  // ---- the public catalog ----
  r = await pub.get('/public/shop');
  ok('shop info', r.status === 200 && r.data.name === `Acme Store ${stamp}` && r.data.categories.some((c: any) => c.slug === mugs.slug) && r.data.terms, r.data);
  r = await pub.get(`/public/shop/products?q=${stamp}`);
  const names = r.data.items.map((p: any) => p.name).sort();
  ok('only listed, active products', JSON.stringify(names) === JSON.stringify([`Mug ${stamp}`, `Tee ${stamp}`]), names);
  const pm = r.data.items.find((p: any) => p.slug === mug.slug);
  ok('price shown with tax, stock and photo', Number(pm.price) === 12 && Number(pm.stock) === 3 && pm.image === `/api/public/shop/images/${mug.slug}` && pm.category === mugs.slug, pm);
  r = await pub.get(`/public/shop/products?category=${tees.slug}`);
  ok('category filter', r.data.items.length === 1 && r.data.items[0].slug === tee.slug, r.data);
  ok('product page', (await pub.get(`/public/shop/products/${mug.slug}`)).data.product?.description === 'A big mug');
  ok('unlisted product not found', (await pub.get(`/public/shop/products/${hidden.slug ?? 'backroom'}`)).status === 404);
  const img = await fetch(`http://localhost:8080/api/public/shop/images/${mug.slug}`);
  ok('photo served publicly', img.status === 200 && img.headers.get('content-type') === 'image/png' && /max-age/.test(img.headers.get('cache-control') ?? '') && (await img.arrayBuffer()).byteLength === PNG.length);
  ok('no photo, no image', (await fetch(`http://localhost:8080/api/public/shop/images/${tee.slug}`)).status === 404);

  // ---- checkout ----
  const buyer = `kim.${stamp}@example.com`;
  const order = (b: any) => pub.post('/public/shop/checkout', { name: `Kim Lee ${stamp}`, email: buyer, address: '1 Main St\nSpringfield', accept_terms: true, ...b });
  ok('empty cart', (await order({ items: [] })).status === 400);
  ok('bad email', (await order({ email: 'kim@nowhere', items: [{ slug: tee.slug, quantity: 1 }] })).status === 400);
  ok('address needed', (await order({ address: ' ', items: [{ slug: tee.slug, quantity: 1 }] })).status === 400);
  ok('whole quantities only', (await order({ items: [{ slug: tee.slug, quantity: 1.5 }] })).status === 400 && (await order({ items: [{ slug: tee.slug, quantity: 0 }] })).status === 400);
  ok('terms must be accepted', (await order({ accept_terms: false, items: [{ slug: tee.slug, quantity: 1 }] })).status === 400);
  ok('unlisted product cannot be ordered', (await order({ items: [{ slug: old.slug, quantity: 1 }] })).status === 400);
  r = await order({ items: [{ slug: mug.slug, quantity: 4 }] });
  ok('not more than in stock', r.status === 400 && /only 3/.test(JSON.stringify(r.data)), r.data);
  r = await order({ website: 'http://spam', items: [{ slug: tee.slug, quantity: 1 }] });
  ok('bots get a fake success', r.status === 200 && r.data.number === 0);
  r = await order({ phone: '+1 555 0100', note: 'Ring twice', items: [{ slug: mug.slug, quantity: 1 }, { slug: tee.slug, quantity: 1 }, { slug: mug.slug, quantity: 1 }] });
  ok('order placed', r.status === 200 && r.data.token.length >= 30 && r.data.number > 0, r.data);
  const { token, number } = r.data;
  ok('stock taken', Number((await pub.get(`/public/shop/products/${mug.slug}`)).data.product.stock) === 1);

  // The order record.
  const list = (await admin.get(`/crm/order/items?q=${stamp}`)).data.items;
  const rec = list.find((i: any) => i.number === number);
  ok('order record', rec && rec.title === `Order ${number}: Kim Lee ${stamp}` && Number(rec.opportunity) === 29 && rec.assigned_to === samUser.id, list);
  const det = (await admin.get(`/crm/order/items/${rec.id}`)).data;
  ok('linked to a new contact', det.contacts.length === 1 && /Kim Lee/.test(det.contacts[0].title) && /1 Main St/.test(det.comments) && /Ring twice/.test(det.comments), det);
  const rows = (await admin.get(`/crm/items/${rec.id}/products`)).data;
  ok('products as rows (same product added up)', rows.rows.length === 2 && Number(rows.rows[0].row.quantity) === 2 && rows.rows[0].row.product_id === mug.id && Number(rows.totals.tax) === 4, rows);
  const contact = (await admin.get(`/crm/contact/items/${det.contacts[0].id}`)).data;
  ok('contact has email and phone', contact.first_name === 'Kim Lee' && contact.last_name === stamp && contact.multifields.some((m: any) => m.value === buyer) && contact.multifields.some((m: any) => m.kind === 'phone'), contact);
  const tl = (await admin.get(`/crm/items/${rec.id}/timeline`)).data.items;
  ok('timeline says it came from the shop', tl.some((e: any) => e.kind === 'shop_order' && e.data.total === '29.00'), tl.map((e: any) => e.kind));
  r = await admin.get(`/shop/orders/${rec.id}`);
  ok('checkout details for staff', r.data.address === '1 Main St\nSpringfield' && r.data.phone === '+1 555 0100' && r.data.order_url.endsWith(`/shop/order/${token}`), r.data);
  ok('checkout details follow CRM access', (await new Client().get(`/shop/orders/${rec.id}`)).status === 401);

  const note = await waitFor(async () => (await sam.get('/notifications')).data.items?.find((n: any) => n.title.includes(`order ${number}`)), 10000);
  ok('handler notified', note?.link === `/crm/order/${rec.id}`, note);
  const conf = await waitFor(async () => (await mailpit(`to:${buyer} subject:"Your order ${number}"`))[0], 15000);
  const confText: string = conf ? (await (await fetch(`${MP}/message/${conf.ID}`)).json()).Text : '';
  ok('confirmation email with payment details', /2 × Mug/.test(confText) && /Total: 29\.00/.test(confText) && confText.includes(`reference "Order ${number}"`) && confText.includes(`/shop/order/${token}`), confText);

  // The customer's order page.
  r = await pub.get(`/public/shop/orders/${token}`);
  ok('order page', r.status === 200 && r.data.number === number && r.data.stage.name === 'New' && r.data.steps[0].done && !r.data.steps[1].done && r.data.payment_instructions?.includes('Transfer 29.00') && r.data.rows.length === 2, r.data);
  ok('unknown order link', (await pub.get('/public/shop/orders/nope')).status === 404);

  // Same customer again: same contact.
  r = await order({ email: buyer.toUpperCase(), items: [{ slug: tee.slug, quantity: 1 }] });
  const second = (await admin.get(`/crm/order/items?q=${stamp}`)).data.items.find((i: any) => i.number === r.data.number);
  const det2 = (await admin.get(`/crm/order/items/${second.id}`)).data;
  ok('returning customer keeps their contact', det2.contacts[0]?.id === det.contacts[0].id, det2.contacts);
  r = await order({ items: [{ slug: mug.slug, quantity: 2 }] });
  ok('only what is left', r.status === 400 && /only 1/.test(JSON.stringify(r.data)), r.data);

  // ---- stages ----
  const cfg = (await admin.get('/crm/config')).data;
  const stages = cfg.types.find((t: any) => t.kind === 'order').categories[0].stages;
  const st = (n: string) => stages.find((s: any) => s.name === n).id;
  await admin.post(`/crm/order/items/${rec.id}/move`, { stage_id: st('Paid') });
  r = await pub.get(`/public/shop/orders/${token}`);
  ok('paid: progress moves, no payment details', r.data.stage.name === 'Paid' && r.data.steps[1].done && r.data.payment_instructions === null, r.data);
  ok('customer told', !!(await waitFor(async () => (await mailpit(`to:${buyer} subject:"Order ${number}: Paid"`))[0], 15000)));
  await admin.post(`/crm/order/items/${rec.id}/move`, { stage_id: st('Cancelled') });
  ok('cancelled: stock back', Number((await pub.get(`/public/shop/products/${mug.slug}`)).data.product.stock) === 3);
  ok('customer told about the cancellation', !!(await waitFor(async () => (await mailpit(`to:${buyer} subject:"Order ${number} was cancelled"`))[0], 15000)));
  ok('order page shows it', (await pub.get(`/public/shop/orders/${token}`)).data.stage.semantics === 'failure');
  ok('staff see the stock is back', (await admin.get(`/shop/orders/${rec.id}`)).data.stock_returned === true);
  await admin.post(`/crm/order/items/${rec.id}/move`, { stage_id: st('Shipped') });
  ok('reopened: stock taken again', Number((await pub.get(`/public/shop/products/${mug.slug}`)).data.product.stock) === 1);
  await admin.post(`/crm/order/items/${rec.id}/move`, { stage_id: st('Completed') });
  ok('completed: stock stays', Number((await pub.get(`/public/shop/products/${mug.slug}`)).data.product.stock) === 1);

  // ---- an invoice from the order ----
  r = await admin.post(`/crm/invoice/items/from/${rec.id}`, {});
  ok('invoice from an order', r.status === 200 && r.data.source_item_id === rec.id && Number(r.data.opportunity) === 29, r.data);

  // ---- closing ----
  await admin.patch('/shop/settings', { enabled: false });
  ok('closed: catalog gone, order links still work', (await pub.get('/public/shop/products')).status === 404 && (await pub.get(`/public/shop/orders/${token}`)).status === 200);
  ok('closed: no checkout', (await order({ items: [{ slug: tee.slug, quantity: 1 }] })).status === 404);
} finally {
  await admin.patch('/shop/settings', { ...before, handlers: before.handlers.filter((h: string) => h !== samUser.id) });
  await cleanupRun(stamp);
}

console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
