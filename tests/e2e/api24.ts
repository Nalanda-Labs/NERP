// Catalog depth: product properties (validation, values), price types (prices, prices for
// product rows), variants (making them, skipping existing ones, editing one), the product
// list order, and variants in the shop (listing, product page, checkout).
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 700)); };
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const pub = new Client();
const base = (await admin.get('/crm/config')).data.base_currency;
const shopBefore = (await admin.get('/shop/settings')).data.settings;
const prop = async (b: any) => (await admin.post('/crm/product-props', b)).data.items.find((p: any) => p.name === b.name);

try {
  // ---- properties ----
  ok('only lists make variants', (await admin.post('/crm/product-props', { name: `Bad ${stamp}`, kind: 'text', is_variant: true })).status === 400);
  ok('a list needs values', (await admin.post('/crm/product-props', { name: `Bad2 ${stamp}`, kind: 'list', options: [' '] })).status === 400);
  const material = await prop({ name: `Material ${stamp}`, kind: 'text' });
  const weight = await prop({ name: `Weight ${stamp}`, kind: 'number', in_shop: false });
  const size = await prop({ name: `Size ${stamp}`, kind: 'list', options: ['S', 'M', 'L', 'M'], is_variant: true });
  const colour = await prop({ name: `Colour ${stamp}`, kind: 'list', options: ['Red', 'Blue'], is_variant: true });
  ok('properties made', material && weight && size?.options.join() === 'S,M,L' && colour?.is_variant, size);
  ok('kind cannot change', (await admin.patch(`/crm/product-props/${weight.id}`, { kind: 'text' })).status === 400);

  // ---- price types ----
  const wholesale = (await admin.post('/crm/price-types', { name: `Wholesale ${stamp}` })).data.items.find((t: any) => t.name === `Wholesale ${stamp}`);
  ok('price type made', !!wholesale);

  // ---- a product ----
  const tee = (await admin.post('/crm/products', { name: `Tee ${stamp}`, price: '20', currency: base, in_shop: true, description: 'Soft tee' })).data;
  let r = await admin.put(`/crm/products/${tee.id}/props`, { props: { [material.id]: 'Cotton', [weight.id]: '0,2' } });
  ok('property values stored', r.status === 200 && r.data.props[material.id] === 'Cotton' && r.data.props[weight.id] === 0.2, r.data);
  ok('numbers checked', (await admin.put(`/crm/products/${tee.id}/props`, { props: { [weight.id]: 'heavy' } })).status === 400);
  ok('variant values come from variants', (await admin.put(`/crm/products/${tee.id}/props`, { props: { [size.id]: 'M' } })).status === 400);
  r = await admin.put(`/crm/products/${tee.id}/prices`, { prices: { [wholesale.id]: '15' } });
  ok('extra price stored', Number(r.data.prices[wholesale.id]) === 15);
  ok('negative prices refused', (await admin.put(`/crm/products/${tee.id}/prices`, { prices: { [wholesale.id]: '-1' } })).status === 400);
  const mug = (await admin.post('/crm/products', { name: `Mug ${stamp}`, price: '8', currency: base })).data;
  r = await admin.post('/crm/prices', { price_type_id: wholesale.id, product_ids: [tee.id, mug.id] });
  ok('prices for rows: the type, or the base price', Number(r.data.prices[tee.id]) === 15 && Number(r.data.prices[mug.id]) === 8, r.data);

  // ---- variants ----
  ok('values must belong to the property', (await admin.post(`/crm/products/${tee.id}/variants`, { values: { [size.id]: ['XXL'] } })).status === 400);
  ok('only variant properties', (await admin.post(`/crm/products/${tee.id}/variants`, { values: { [material.id]: ['Cotton'] } })).status === 400);
  r = await admin.post(`/crm/products/${tee.id}/variants`, { values: { [size.id]: ['S', 'M'], [colour.id]: ['Red'] } });
  ok('variants made', r.status === 200 && r.data.made === 2 && r.data.variants.map((v: any) => v.name).sort().join() === [`Tee ${stamp} (M, Red)`, `Tee ${stamp} (S, Red)`].join(), r.data);
  const vM = r.data.variants.find((v: any) => v.name.includes('(M, Red)'));
  ok('variants copy price and the product\'s properties', Number(vM.price) === 20 && vM.props[material.id] === 'Cotton' && vM.props[size.id] === 'M', vM);
  r = await admin.post(`/crm/products/${tee.id}/variants`, { values: { [size.id]: ['S', 'M', 'L'], [colour.id]: ['Red'] } });
  ok('existing combinations skipped', r.data.made === 1 && r.data.variants.length === 3, r.data);
  ok('a variant has no variants', (await admin.post(`/crm/products/${vM.id}/variants`, { values: { [size.id]: ['S'] } })).status === 400);
  const vDetails = (await admin.get(`/crm/products/${vM.id}/details`)).data;
  ok('a variant knows its parent', vDetails.parent?.id === tee.id);
  r = await admin.patch(`/crm/products/${vM.id}`, { name: vM.name, price: '22', currency: base, stock: '3' });
  ok('a variant edited on its own', Number(r.data.price) === 22 && Number(r.data.stock) === 3 && r.data.parent_id === tee.id, r.data);
  const list = (await admin.get(`/crm/products?q=${stamp}`)).data.items.map((p: any) => p.name);
  ok('variants listed after their parent', list.indexOf(`Tee ${stamp}`) === list.indexOf(`Tee ${stamp} (L, Red)`) - 1, list);
  ok('a property variants use cannot be deleted', (await admin.req('DELETE', `/crm/product-props/${size.id}`)).status === 400);
  ok('or stop making variants', (await admin.patch(`/crm/product-props/${size.id}`, { is_variant: false })).status === 400);

  // ---- in the shop ----
  await admin.patch('/shop/settings', { enabled: true, currency: base, payment_instructions: 'Pay by transfer', terms: null });
  r = await pub.get(`/public/shop/products?q=${stamp}`);
  const listed = r.data.items.find((p: any) => p.name === `Tee ${stamp}`);
  ok('listed once, from the lowest price', r.data.items.filter((p: any) => p.name.startsWith(`Tee ${stamp}`)).length === 1 && listed.variants === 3 && Number(listed.price) === 20 && listed.stock === null, r.data.items);
  r = await pub.get(`/public/shop/products/${tee.slug ?? listed.slug}`);
  ok('product page: choices and details', r.data.axes.length === 2 && r.data.axes[0].values.join() === 'S,M,L' && r.data.axes[1].values.join() === 'Red' && r.data.props.length === 1 && r.data.props[0].value === 'Cotton' && r.data.variants.length === 3 && r.data.product.description === 'Soft tee', r.data);
  const pv = r.data.variants.find((v: any) => v.values[size.id] === 'M');
  ok('a variant: its own price and stock, the parent\'s description', Number(pv.price) === 22 && Number(pv.stock) === 3 && pv.description === 'Soft tee', pv);
  ok('a variant\'s address opens its parent with it chosen', (await pub.get(`/public/shop/products/${pv.slug}`)).data.selected === pv.slug);
  const buy = (slug: string, quantity: number) => pub.post('/public/shop/checkout', { name: `Vic ${stamp}`, email: `vic.${stamp}@example.com`, address: 'Here', items: [{ slug, quantity }] });
  r = await buy(listed.slug, 1);
  ok('the parent itself cannot be ordered', r.status === 400 && /choose which/.test(JSON.stringify(r.data)), r.data);
  r = await buy(pv.slug, 2);
  ok('a variant can', r.status === 200 && Number((await admin.get(`/crm/products/${vM.id}`)).data.stock) === 1, r.data);

  // ---- deleting ----
  r = await admin.req('DELETE', `/crm/products/${tee.id}`);
  const after = (await admin.get(`/crm/products?q=${stamp}&include_inactive=true`)).data.items.filter((p: any) => p.name.startsWith(`Tee ${stamp}`));
  ok('a product whose variant was sold is deactivated with its variants', r.data.deactivated && after.length === 4 && after.every((p: any) => !p.is_active), after);
} finally {
  await admin.patch('/shop/settings', shopBefore);
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
