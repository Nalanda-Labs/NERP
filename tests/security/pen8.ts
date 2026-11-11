// Business logic and the public pages: prices and quantities a shop visitor sends, coupons
// and room bookings raced from many requests at once, read-only links that try to write,
// join links with look-alike email domains, sign-in redirects to other sites, forged
// client addresses (X-Forwarded-For) against rate limits, and a forged Host header against
// the websites on their own domains.
import { admin, person, raw, ok, finish, resetLimits, stamp, BASE } from './lib.ts';

const json = (o: unknown) => ({ headers: { 'content-type': 'application/json' }, body: JSON.stringify(o) });
const all = <T>(n: number, f: (i: number) => Promise<T>) => Promise.all(Array.from({ length: n }, (_, i) => f(i)));
const made = { delivery: [] as string[], coupons: [] as string[], resources: [] as string[] };
const shopBefore = (await admin.get('/shop/settings')).data.settings;

try {
  // ---------------------------------------------------------------------------
  // The shop: the server sets prices; quantities are checked; coupons can't be raced
  // ---------------------------------------------------------------------------
  const base = (await admin.get('/crm/config')).data.base_currency;
  const prod = (await admin.post('/crm/products', { name: `Pen bag ${stamp}`, price: '10', currency: base })).data;
  await admin.patch(`/crm/products/${prod.id}`, { name: prod.name, price: '10', currency: base, in_shop: true });
  const slug = (await admin.get(`/crm/products/${prod.id}`)).data.slug;
  const hidden = (await admin.post('/crm/products', { name: `Pen hidden ${stamp}`, price: '1', currency: base })).data;
  const hiddenSlug = (await admin.get(`/crm/products/${hidden.id}`)).data.slug;
  await admin.patch('/shop/settings', { enabled: true, payment_instructions: 'Pay by transfer', terms: null, warehouse_id: null });
  const pickup = (await admin.post('/shop/delivery', { name: `Pen pick-up ${stamp}`, price: '0', needs_address: false })).data.items.find((d: any) => d.name === `Pen pick-up ${stamp}`);
  made.delivery.push(pickup.id);
  const checkout = (b: object) => raw('POST', '/public/shop/checkout', json({ name: `Pen buyer ${stamp}`, email: `pen.${stamp}@example.com`, address: '', delivery_id: pickup.id, items: [{ slug, quantity: 1 }], ...b }));
  const orderTotal = async (email: string) => {
    const items = (await admin.get(`/crm/order/items?q=${encodeURIComponent(email)}`)).data.items ?? [];
    return items.map((i: any) => Number(i.opportunity));
  };

  let r = await checkout({ email: `price.${stamp}@example.com`, items: [{ slug, quantity: 1, price: '0.01', total: '0.01', discount: '100' }], total: '0.01', subtotal: '0.01' });
  ok('prices sent by the visitor are ignored', r.status === 200 && (await orderTotal(`price.${stamp}@example.com`)).every((t: number) => t === 10), [r.status, r.json, await orderTotal(`price.${stamp}@example.com`)]);
  const qty: string[] = [];
  for (const q of [0, -1, 1.5, 1e9, 'NaN', null, '1; DROP TABLE users']) {
    const x = await raw('POST', '/public/shop/quote', json({ items: [{ slug, quantity: q }] }));
    if (x.status === 200 && !(Number(x.json?.total) > 0)) qty.push(`${q} → ${x.status} ${x.text.slice(0, 80)}`);
    if (x.status >= 500) qty.push(`${q} → ${x.status}`);
  }
  ok('odd quantities are refused, never priced at zero or less', qty.length === 0, qty);
  r = await raw('POST', '/public/shop/quote', json({ items: [{ slug: hiddenSlug, quantity: 5 }] }));
  ok('products not in the shop are not sold', r.status >= 400 || Number(r.json?.subtotal) === 0, r.json);
  r = await checkout({ email: `hidden.${stamp}@example.com`, items: [{ slug: hiddenSlug, quantity: 5 }] });
  ok('…nor ordered', r.status >= 400, r.status);
  r = await raw('POST', '/public/shop/quote', json({ items: [{ slug, quantity: 999 }, { slug, quantity: 999 }, { slug, quantity: 999 }] }));
  ok('the same product many times is still one priced line each', r.status >= 400 || Number(r.json?.subtotal) === 29970, r.json);
  resetLimits('shop:checkout:*');

  // A coupon once per customer, raced.
  const once = (await admin.post('/shop/coupons', { code: `PENONCE${stamp}`, kind: 'percent', value: '50', once_per_customer: true })).data;
  made.coupons.push(once.id);
  const racer = `racer.${stamp}@example.com`;
  const raced = await all(6, () => checkout({ email: racer, coupon: `PENONCE${stamp}` }));
  ok('a once-per-customer coupon used in 6 orders at once works once', raced.filter((x) => x.status === 200).length === 1, raced.map((x) => `${x.status} ${x.json?.error?.message ?? ''}`));
  resetLimits('shop:checkout:*');
  const capped = (await admin.post('/shop/coupons', { code: `PENCAP${stamp}`, kind: 'percent', value: '50', max_uses: 2 })).data;
  made.coupons.push(capped.id);
  const many = await all(8, (i) => checkout({ email: `cap${i}.${stamp}@example.com`, coupon: `PENCAP${stamp}` }));
  ok('a coupon for 2 uses raced by 8 orders is used twice', many.filter((x) => x.status === 200).length === 2, many.map((x) => x.status));
  resetLimits('shop:checkout:*');
  const order = raced.find((x) => x.status === 200)?.json;
  if (order?.token) {
    ok('an order page needs its own token', (await raw('GET', `/public/shop/orders/${order.token.slice(0, -2)}xx`)).status === 404);
  }

  // ---------------------------------------------------------------------------
  // A room for one, booked from many requests at once
  // ---------------------------------------------------------------------------
  const booker = await person('Bea', ['booking.manage']);
  const room = (await admin.post('/booking/resources', { name: `Pen room ${stamp}`, kind: 'room', capacity: 1, timezone: 'UTC', min_notice_minutes: 0 })).data;
  made.resources.push(room.id);
  const day = new Date(Date.now() + 9 * 86400e3);
  day.setUTCHours(10, 0, 0, 0);
  const at = day.toISOString(), until = new Date(+day + 3600e3).toISOString();
  const bookings = await all(6, (i) => booker.post('/booking/bookings', { resource_ids: [room.id], starts_at: at, ends_at: until, client_name: `Pen client ${i} ${stamp}` }));
  const late = await booker.post('/booking/bookings', { resource_ids: [room.id], starts_at: at, ends_at: until, client_name: `Pen late ${stamp}` });
  ok('(one more, afterwards, is refused)', late.status === 409, [late.status, late.data]);
  ok('a room for one, booked 6 times at once, is booked once', bookings.filter((x: any) => x.status === 200).length === 1, bookings.map((x: any) => `${x.status} ${x.data?.error?.message ?? ''}`));

  // ---------------------------------------------------------------------------
  // Read-only links stay read-only
  // ---------------------------------------------------------------------------
  const ann = await person('Ann');
  const plan = (await ann.post('/boards', { title: `Pen board ${stamp}` })).data;
  const view = (await ann.post(`/boards/${plan.id}/links`, { level: 'view' })).data;
  const vt = (view.url ?? view.link ?? '').split('/').pop() || view.token;
  ok('a view link reads the board', (await raw('GET', `/public/boards/${vt}`)).status === 200, view);
  const writes = [
    await raw('PUT', `/public/boards/${vt}/scene`, json({ elements: [{ id: 'x', type: 'rectangle' }] })),
    await raw('POST', `/public/boards/${vt}/files`, json({ key: 'pwn', data_url: 'data:image/png;base64,iVBORw0KGgo=' })),
  ];
  ok('…but can\'t change it or add files', writes.every((w) => w.status === 403 || w.status === 401), writes.map((w) => w.status));

  // ---------------------------------------------------------------------------
  // Join links: only the allowed email domain, and no taking over accounts
  // ---------------------------------------------------------------------------
  const link = (await admin.post('/join-links', { note: `Pen ${stamp}`, email_domains: ['acme.test'], needs_approval: true })).data;
  const jt = (link.url ?? '').split('/join/')[1] ?? link.token;
  const lookalikes = [`x.${stamp}@acme.test.evil.com`, `x.${stamp}@evilacme.test`, `x.${stamp}@acme.test@evil.com`, `"x@acme.test"@evil.com`, `x.${stamp}@ACME.TEST.evil.com`, `x.${stamp}@acme.test\u0000.evil.com`];
  const let_in: string[] = [];
  for (const email of lookalikes) {
    const x = await raw('POST', `/public/join/${jt}`, json({ email, first_name: 'Eve', last_name: `Pen${stamp}` }));
    if (x.status === 200) let_in.push(`${email} → ${x.status}`);
  }
  ok('look-alike email domains are refused', let_in.length === 0, let_in);
  resetLimits('join:*');
  const before = (await admin.get('/auth/me')).data;
  r = await raw('POST', `/public/join/${jt}`, json({ email: 'ADMIN@acme.test', first_name: 'Pwned', last_name: 'Admin' }));
  const after = (await admin.get('/auth/me')).data;
  ok('joining with an existing person\'s email changes nothing about them', r.status === 200 && after.first_name === before.first_name && after.email === before.email, [r.status, after.first_name]);
  ok('…and the admin can still sign in', (await raw('POST', '/auth/login', json({ email: 'admin@acme.test', password: 'supersecret1' }))).status === 200);
  resetLimits('join:*');

  // ---------------------------------------------------------------------------
  // Redirects never send people to other sites
  // ---------------------------------------------------------------------------
  const evil = ['https://evil.example/x', '//evil.example/x', '/\\evil.example', 'javascript:alert(1)', '\\\\evil.example', '/%2F%2Fevil.example', 'https:evil.example'];
  const bad: string[] = [];
  for (const target of evil) {
    for (const path of [`/auth/sso/start?next=${encodeURIComponent(target)}`, `/auth/sso/start?return_to=${encodeURIComponent(target)}`, `/auth/providers/00000000-0000-4000-8000-000000000000/start?next=${encodeURIComponent(target)}`]) {
      const x = await raw('GET', path);
      const loc = x.headers.get('location') ?? '';
      if (/^(https?:)?\/\/evil|^\/\\|^\\\\|^javascript:|evil\.example/i.test(loc) && !/accounts\.|login\.|appleid|facebook|twitter|oauth/i.test(loc.split('?')[0])) bad.push(`${path} → ${loc}`);
    }
  }
  ok('sign-in starts never redirect to another site', bad.length === 0, bad);
  const click = await raw('GET', `/public/m/c/nope?u=${encodeURIComponent('https://evil.example')}&s=forged`);
  ok('campaign click links don\'t redirect anywhere unsigned', !(click.headers.get('location') ?? '').includes('evil.example'), click.status);

  // ---------------------------------------------------------------------------
  // Forged client addresses
  // ---------------------------------------------------------------------------
  const victim = await person('Vic');
  let locked = false;
  for (let i = 0; i < 25 && !locked; i++) {
    const x = await raw('POST', '/auth/login', { headers: { 'content-type': 'application/json', 'x-forwarded-for': `203.0.113.${i}`, 'x-real-ip': `198.51.100.${i}` }, body: JSON.stringify({ email: victim.email, password: `wrong-${i}` }) });
    if (x.status === 429) locked = true;
  }
  ok('a new X-Forwarded-For on every try doesn\'t dodge the sign-in limit', locked);
  resetLimits(`auth:fail:${victim.email}`);
  resetLimits('auth:failip:*');
  // The company IP rule: a forged header must not count as an allowed address.
  const emp = await person('Ira');
  const ipBefore = (await admin.get('/settings')).data;
  const setIp = await admin.put('/settings', { ip_allow: ['203.0.113.0/24'], ip_allow_admins_exempt: true });
  ok('(the rule is saved)', setIp.status === 200, setIp.data);
  const blocked = await raw('GET', '/auth/me', { headers: { authorization: `Bearer ${emp.token}`, 'x-forwarded-for': '203.0.113.7', 'x-real-ip': '203.0.113.7', forwarded: 'for=203.0.113.7' } });
  const plain = await raw('GET', '/auth/me', { headers: { authorization: `Bearer ${emp.token}` } });
  await admin.put('/settings', { ip_allow: ipBefore.ip_allow ?? [], ip_allow_admins_exempt: ipBefore.ip_allow_admins_exempt ?? true });
  ok('(from outside the range, an employee is refused)', plain.status === 403, plain.status);
  ok('the IP rule can\'t be passed by claiming an allowed address in a header', blocked.status === 403, blocked.status);

  // ---------------------------------------------------------------------------
  // Websites on their own domains: the Host header
  // ---------------------------------------------------------------------------
  const sd = await raw('GET', '/public/site-domain', { headers: { host: `nobody-${stamp}.example` } });
  ok('an unknown Host is no site', sd.status === 404 || !sd.json?.slug, sd.json);
  const forgedHost = await raw('GET', '/public/site-domain', { headers: { 'x-forwarded-host': 'localhost:8080' } });
  ok('X-Forwarded-Host is not trusted', forgedHost.status === 404 || !forgedHost.json?.slug, forgedHost.json);
} finally {
  for (const id of made.coupons) await admin.del(`/shop/coupons/${id}`);
  for (const id of made.delivery) await admin.del(`/shop/delivery/${id}`);
  for (const id of made.resources) await admin.del(`/booking/resources/${id}`);
  await admin.patch('/shop/settings', shopBefore);
  resetLimits('shop:checkout:*');
}
void BASE;
await finish();
