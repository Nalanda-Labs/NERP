// Websites and the shop, in depth. Sites: starting templates, an HTML block in its own
// sandbox, a products block, copying pages and sites, reusable groups of blocks, export and
// import, a site's own domain (and the CSP there), Google Analytics and the cookie banner.
// Shop: ways of delivery (free above an amount, pick-up without an address), coupons (a
// percentage or an amount, a minimum, used up, once per customer), loyalty discounts, the
// cart's quote, orders carrying all of it, and the shop's set-up exported and imported.
import http from 'node:http';
import { execFileSync } from 'node:child_process';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 900)); };
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const pub = new Client();
const upload = async (name: string, bytes: Uint8Array, type: string) => {
  const fd = new FormData();
  fd.append('file', new Blob([bytes as BlobPart], { type }), name);
  return (await admin.req('POST', '/files', fd)).data;
};
// A 1×1 PNG.
const PNG = Uint8Array.from(Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==', 'base64'));
const shopBefore = (await admin.get('/shop/settings')).data.settings;
const loyaltyBefore = (await admin.get('/shop/loyalty')).data.tiers;
const made: { sites: string[]; coupons: string[]; delivery: string[]; groups: string[] } = { sites: [], coupons: [], delivery: [], groups: [] };

try {
  // ================= websites =================
  ok('templates are known', (await admin.post('/sites', { name: `X ${stamp}`, slug: `x-${stamp}`, template: 'space' })).status === 400);
  let r = await admin.post('/sites', { name: `Sales ${stamp}`, slug: `sales-${stamp}`, template: 'sales' });
  const site = r.data;
  made.sites.push(site.id);
  ok('a site from the sales template', r.status === 200 && ['', 'prices', 'contact'].every((p) => site.page_list.some((x: any) => x.path === p)), r.data);
  const home = site.page_list.find((x: any) => x.path === '');
  const img = await upload('logo.png', PNG, 'image/png');
  r = await admin.patch(`/sites/pages/${home.id}`, { blocks: [
    { kind: 'hero', heading: 'Hi' },
    { kind: 'html', html: `<div id="w">Widget ${stamp}</div><script>document.getElementById('w').textContent += '!'</script>`, height: 120 },
    { kind: 'products', heading: 'Buy', limit: 4 },
    { kind: 'image', image: img.id },
  ] });
  ok('HTML, products and image blocks', r.status === 200 && r.data.blocks[1].kind === 'html' && r.data.blocks[2].limit === 4, r.data);
  ok('blocks are checked', (await admin.patch(`/sites/pages/${home.id}`, { blocks: [{ kind: 'products', limit: 0 }] })).status === 400
    && (await admin.patch(`/sites/pages/${home.id}`, { blocks: [{ kind: 'html', html: 'x', height: 5 }] })).status === 400);
  await admin.post(`/sites/${site.id}/publish`, {});
  r = await pub.get(`/public/sites/sales-${stamp}/page?path=`);
  const htmlBlock = r.data.blocks[1];
  ok('visitors get the HTML block as a sandboxed address, not its code', htmlBlock.embed_url?.includes('/embed/') && !('html' in htmlBlock), htmlBlock);
  let res = await pub.req('GET', htmlBlock.embed_url.replace(/^\/api/, ''), undefined, true);
  const csp = res.headers.get('content-security-policy') ?? '';
  ok('which runs in a sandbox of its own', res.status === 200 && /^sandbox allow-scripts/.test(csp) && !csp.includes('allow-same-origin') && (await res.text()).includes(`Widget ${stamp}`), csp);
  ok('only HTML blocks of published pages', (await pub.req('GET', htmlBlock.embed_url.replace(/^\/api/, '').replace(/\/1$/, '/0'), undefined, true)).status === 404);

  // Copies.
  r = await admin.post(`/sites/pages/${home.id}/copy`);
  ok('a page copied', r.status === 200 && r.data.path === 'home-copy' && r.data.title === 'Home (copy)' && !r.data.published_at && r.data.blocks.length === 4, r.data);
  r = await admin.post(`/sites/pages/${home.id}/copy`);
  ok('and again', r.data.path === 'home-copy-2', r.data.path);
  r = await admin.post(`/sites/${site.id}/copy`, {});
  made.sites.push(r.data.id);
  ok('a whole site copied', r.status === 200 && r.data.slug === `sales-${stamp}-copy` && r.data.page_list.length === site.page_list.length + 2 && r.data.page_list.every((p: any) => !p.published_at), r.data);

  // Reusable groups.
  ok('a group needs blocks', (await admin.post('/sites/saved-blocks', { name: 'x', blocks: [] })).status === 400);
  r = await admin.post('/sites/saved-blocks', { name: `Footer bits ${stamp}`, blocks: [{ kind: 'cta', heading: 'Call us' }, { kind: 'contact', phone: '123' }] });
  const group = r.data.items.find((g: any) => g.name === `Footer bits ${stamp}`);
  made.groups.push(group.id);
  ok('a reusable group of blocks', group.blocks.length === 2, r.data);

  // Own domain, Analytics, cookies.
  const domain = `www.site-${stamp}.test`;
  ok('a domain is checked', (await admin.patch(`/sites/${site.id}`, { domain: 'not a domain' })).status === 400);
  r = await admin.patch(`/sites/${site.id}`, { domain: `HTTPS://${domain.toUpperCase()}/`, settings: { color: '#112233', analytics_id: 'G-TEST123', cookie_banner: true, cookie_text: 'Cookies?' } });
  ok('its own domain', r.status === 200 && r.data.domain === domain, r.data);
  ok('Analytics IDs are checked', (await admin.patch(`/sites/${site.id}`, { settings: { analytics_id: 'UA-1' } })).status === 400);
  ok('one domain per site', (await admin.patch(`/sites/${made.sites[1]}`, { domain })).status === 400);
  r = await pub.get(`/public/site-domain?host=${domain}:443`);
  ok('the domain finds its site', r.data.slug === `sales-${stamp}`, r.data);
  ok('other hosts find none', (await pub.get('/public/site-domain?host=nope.test')).status === 404);
  r = await pub.get(`/public/sites/sales-${stamp}`);
  ok('visitors get Analytics and the banner settings', r.data.analytics_id === 'G-TEST123' && r.data.cookie_banner === true && r.data.cookie_text === 'Cookies?', r.data);
  // fetch() won't send another Host header; a plain request does.
  const cspOn = (host: string) => new Promise<string>((done) => http.get({ host: '127.0.0.1', port: 8080, path: '/', headers: { host } }, (res) => { res.resume(); done(String(res.headers['content-security-policy'] ?? '')); }));
  ok('on its own domain Analytics may load', (await cspOn(domain)).includes('https://www.googletagmanager.com'));
  ok('never where NERP itself runs', !(await cspOn('localhost:8080')).includes('googletagmanager'));

  // Export and import.
  res = await admin.req('GET', `/sites/${site.id}/export`, undefined, true);
  const file = await res.json();
  ok('exported with its pictures', file.format === 'nerp-site/1' && file.pages.length === site.page_list.length + 2 && Object.keys(file.files).includes(img.id), Object.keys(file));
  r = await admin.post('/sites/import', { data: file, slug: `imported-${stamp}` });
  made.sites.push(r.data.id);
  ok('imported as a new site', r.status === 200 && r.data.slug === `imported-${stamp}` && r.data.page_list.length === file.pages.length && !r.data.domain, r.data);
  const imported = (await admin.get(`/sites/pages/${r.data.page_list.find((p: any) => p.path === '').id}`)).data;
  const newImg = imported.blocks.find((b: any) => b.kind === 'image').image;
  ok('with its pictures copied', newImg && newImg !== img.id, imported.blocks);
  ok('only site files are imported', (await admin.post('/sites/import', { data: { format: 'other' }, slug: `y-${stamp}` })).status === 400);
  ok('the address must be free', (await admin.post('/sites/import', { data: file, slug: `imported-${stamp}` })).status === 400);

  // ================= the shop =================
  const base = (await admin.get('/crm/config')).data.base_currency;
  const prod = (await admin.post('/crm/products', { name: `Bag ${stamp}`, price: '10', currency: base })).data;
  await admin.patch(`/crm/products/${prod.id}`, { name: prod.name, price: '10', currency: base, in_shop: true });
  const slug = (await admin.get(`/crm/products/${prod.id}`)).data.slug;
  await admin.patch('/shop/settings', { enabled: true, payment_instructions: 'Pay by transfer', terms: null, warehouse_id: null });
  r = await admin.post('/shop/delivery', { name: `Courier ${stamp}`, price: '5', free_from: '50' });
  const courier = r.data.items.find((d: any) => d.name === `Courier ${stamp}`);
  r = await admin.post('/shop/delivery', { name: `Pick-up ${stamp}`, price: '0', needs_address: false });
  const pickup = r.data.items.find((d: any) => d.name === `Pick-up ${stamp}`);
  made.delivery.push(courier.id, pickup.id);
  ok('ways of delivery', !!courier && !!pickup && (await pub.get('/public/shop/delivery')).data.items.some((d: any) => d.id === courier.id));
  ok('prices are checked', (await admin.post('/shop/delivery', { name: 'x', price: '-1' })).status === 400);
  const coupon = async (b: object) => { const x = await admin.post('/shop/coupons', b); if (x.status === 200) made.coupons.push(x.data.id); return x; };
  r = await coupon({ code: `spring${stamp}`, kind: 'percent', value: '10', min_total: '15' });
  const SPRING = `SPRING${stamp.toUpperCase()}`;
  ok('a coupon (codes in capitals)', r.status === 200 && r.data.code === SPRING, r.data);
  ok('codes are unique', (await coupon({ code: `spring${stamp}`, kind: 'percent', value: '5' })).status === 400);
  ok('a percentage is at most 100', (await coupon({ code: `x${stamp}`, kind: 'percent', value: '150' })).status === 400);
  await coupon({ code: `five${stamp}`, kind: 'amount', value: '5', once_per_customer: true });
  await coupon({ code: `once${stamp}`, kind: 'percent', value: '20', max_uses: 1 });
  await coupon({ code: `later${stamp}`, kind: 'percent', value: '20', starts_at: new Date(Date.now() + 86400e3).toISOString() });
  await admin.put('/shop/loyalty', { tiers: [{ from: 20, percent: 5 }] });
  ok('loyalty levels are checked', (await admin.put('/shop/loyalty', { tiers: [{ from: 10, percent: 150 }] })).status === 400);

  const cart = (n: number) => [{ slug, quantity: n }];
  r = await pub.post('/public/shop/quote', { items: cart(2), coupon: SPRING.toLowerCase(), delivery_id: courier.id });
  ok('a quote: 10% off and courier', r.status === 200 && Number(r.data.subtotal) === 20 && Number(r.data.discount) === 2 && Number(r.data.delivery_price) === 5 && Number(r.data.total) === 23 && r.data.discount_reason === `Coupon ${SPRING}`, r.data);
  r = await pub.post('/public/shop/quote', { items: cart(6), delivery_id: courier.id });
  ok('free delivery above 50', Number(r.data.delivery_price) === 0 && Number(r.data.total) === 60, r.data);
  r = await pub.post('/public/shop/quote', { items: cart(1), coupon: SPRING });
  ok('a coupon’s minimum order', r.status === 400 && /at least 15/.test(r.data.error.message), r.data);
  ok('a coupon not valid yet', /not valid yet/.test((await pub.post('/public/shop/quote', { items: cart(2), coupon: `later${stamp}` })).data.error?.message));
  ok('an unknown coupon', /no coupon/.test((await pub.post('/public/shop/quote', { items: cart(2), coupon: 'NOPE' })).data.error?.message));

  const buyer = `buyer.${stamp}@example.com`;
  const order = (b: object) => pub.post('/public/shop/checkout', { name: `Buyer ${stamp}`, email: buyer, address: 'Main St 1', items: cart(2), ...b });
  ok('a way of delivery must be chosen', /choose a way of delivery/.test((await order({})).data.error?.message));
  r = await order({ delivery_id: courier.id, coupon: `five${stamp}` });
  ok('an order with a coupon and delivery', r.status === 200 && r.data.number > 0, r.data);
  const rec = (await admin.get(`/crm/order/items?q=${encodeURIComponent(`Buyer ${stamp}`)}`)).data.items[0];
  const det = (await admin.get(`/crm/items/${rec.id}/products`)).data;
  ok('its rows: discounted products and the delivery', det.rows?.length === 2 && Number(det.rows[0].row.discount_value) === 25 && det.rows.some((p: any) => p.row.name === `Delivery: Courier ${stamp}` && Number(p.row.price) === 5) && Number(det.totals.total) === 20, det);
  r = await order({ delivery_id: courier.id, coupon: `five${stamp}` });
  ok('once per customer', r.status === 400 && /already used/.test(r.data.error.message), r.data);
  r = await order({ delivery_id: pickup.id, address: '', coupon: `once${stamp}` });
  ok('pick-up needs no address', r.status === 200, r.data);
  r = await pub.post('/public/shop/checkout', { name: 'Other', email: `other.${stamp}@example.com`, address: 'x', items: cart(2), delivery_id: courier.id, coupon: `once${stamp}` });
  ok('a coupon used up', r.status === 400 && /used up/.test(r.data.error.message), r.data);
  r = await pub.post('/public/shop/quote', { items: cart(2), email: buyer });
  ok('loyal customers get their discount', Number(r.data.discount_percent) === 5 && r.data.discount_reason === 'Loyal customer', r.data);
  r = await pub.post('/public/shop/quote', { items: cart(2), email: buyer, coupon: SPRING });
  ok('the larger discount counts', Number(r.data.discount_percent) === 10 && r.data.discount_reason === `Coupon ${SPRING}`, r.data);
  ok('uses are counted', (await admin.get('/shop/coupons')).data.items.find((c: any) => c.code === `ONCE${stamp.toUpperCase()}`)?.uses === 1);

  // The set-up as a file.
  r = await admin.get('/shop/export');
  ok('the shop’s set-up exported', r.data.format === 'nerp-shop/1' && r.data.coupons.some((c: any) => c.code === SPRING && !('uses' in c)) && r.data.delivery.some((d: any) => d.name === `Courier ${stamp}`), Object.keys(r.data));
  const exported = r.data;
  exported.coupons = exported.coupons.filter((c: any) => c.code === SPRING).map((c: any) => ({ ...c, value: '15' }));
  exported.delivery = exported.delivery.filter((d: any) => d.name === `Courier ${stamp}`);
  r = await admin.post('/shop/import', exported);
  ok('and imported again (updating by code and name)', r.status === 200 && r.data.coupons === 1 && r.data.delivery === 1, r.data);
  ok('the coupon took the file’s value', Number((await admin.get('/shop/coupons')).data.items.find((c: any) => c.code === SPRING).value) === 15);
  const del = (await admin.get('/shop/delivery')).data.items.find((d: any) => d.name === `Courier ${stamp}`);
  made.delivery.push(del.id);
  ok('not a shop file', (await admin.post('/shop/import', { format: 'nerp-site/1' })).status === 400);
} finally {
  for (const id of made.coupons) await admin.del(`/shop/coupons/${id}`);
  for (const id of made.delivery) await admin.del(`/shop/delivery/${id}`);
  for (const id of made.groups) await admin.del(`/sites/saved-blocks/${id}`);
  for (const id of made.sites) await admin.del(`/sites/${id}`);
  await admin.put('/shop/loyalty', { tiers: loyaltyBefore ?? [] });
  await admin.patch('/shop/settings', shopBefore);
  // The orders here count against the shop's per-address limit: give the next suites a clean slate.
  try {
    const url = process.env.REDIS_URL ?? 'redis://127.0.0.1:6379';
    const keys = execFileSync('redis-cli', ['-u', url, '--scan', '--pattern', 'shop:checkout:*']).toString().split('\n').filter(Boolean);
    if (keys.length) execFileSync('redis-cli', ['-u', url, 'DEL', ...keys]);
  } catch { /* no redis-cli: the limit passes in ten minutes */ }
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
