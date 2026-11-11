// Websites and the shop in the browser: a careers site from its template with an HTML block
// (running in its sandbox), the site on its own domain (addresses without /s/, the cookie
// banner), and a customer using a coupon and choosing delivery at checkout.
import { chromium } from 'playwright';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
const B = 'http://localhost:8080';
const s = Date.now().toString(36);
const domain = `www.ui-${s}.test`;
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
// The site's own domain reaches this machine.
const browser = await chromium.launch({ executablePath: exe, args: [`--host-resolver-rules=MAP ${domain} 127.0.0.1`] });
const errors: string[] = [];
let n = 0, failed = 0;
const pages: import('playwright').Page[] = [];
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `fail64-${n}-${i}.png`, fullPage: true }); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const shopBefore = (await admin.get('/shop/settings')).data.settings;
const base = (await admin.get('/crm/config')).data.base_currency;
const prod = (await admin.post('/crm/products', { name: `Cap ${s}`, price: '20', currency: base })).data;
await admin.patch(`/crm/products/${prod.id}`, { name: prod.name, price: '20', currency: base, in_shop: true });
const slug = (await admin.get(`/crm/products/${prod.id}`)).data.slug;
await admin.patch('/shop/settings', { enabled: true, payment_instructions: 'Pay by transfer', terms: null });
let siteId = '';

const ctx = async (login: boolean) => {
  const p = await (await browser.newContext({ viewport: { width: 1280, height: 900 }, timezoneId: 'UTC', locale: 'en-GB' })).newPage();
  p.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
  p.on('console', (m) => m.type() === 'error' && !/401|400|403|404|409|Failed to load resource|Cross-Origin-Opener-Policy header has been ignored/.test(m.text()) && errors.push(m.text()));
  pages.push(p);
  if (login) {
    await p.goto(B + '/login'); await p.fill('#email', 'admin@acme.test'); await p.fill('#password', 'supersecret1');
    await p.click('button:has-text("Sign in")'); await p.waitForURL(B + '/');
  }
  return p;
};

try {
  const ada = await ctx(true);

  await step('a careers site from its template', async () => {
    await ada.goto(B + '/sites');
    await ada.click('button:has-text("New website")');
    await ada.fill('#st-n', `Careers ${s}`);
    await ada.check('input[name="tpl"][value="recruitment"]');
    await ada.click('dialog button:has-text("Create")');
    await ada.waitForURL(/\/sites\/[0-9a-f-]{36}$/);
    siteId = ada.url().split('/').pop()!;
    await ada.locator('[data-page="Open positions"]').waitFor();
  });

  await step('an HTML block, published, runs in its sandbox', async () => {
    await ada.click('[data-page="Careers"]');
    await ada.selectOption('select[aria-label="Block to add"]', 'html');
    await ada.click('button:has-text("Add block")');
    await ada.fill('[data-edit-block="html"] textarea[aria-label="HTML"]', `<p id="x">Hello</p><script>document.getElementById('x').textContent = 'Script ran ${s}'</script>`);
    await ada.click('button:has-text("Publish")');
    await ada.locator('[data-state]:has-text("Published")').waitFor();
    await ada.goto(`${B}/s/careers-${s}`);
    await ada.frameLocator('[data-block="html"] iframe').locator(`text=Script ran ${s}`).waitFor();
  });

  await step('the site on its own domain', async () => {
    await admin.patch(`/sites/${siteId}`, { domain, settings: { color: '#0f766e', cookie_banner: true, cookie_text: `Cookies ${s}?` } });
    await admin.post(`/sites/${siteId}/publish`, {});
    const v = await ctx(false);
    await v.goto(`http://${domain}:8080/`);
    await v.locator(`h1:has-text("Work at Careers ${s}")`).waitFor();
    const link = await v.locator('nav[aria-label="Site"] a:has-text("Open positions")').getAttribute('href');
    if (link !== '/jobs') throw new Error(`menu link ${link}`);
    await v.click('nav[aria-label="Site"] a:has-text("Open positions")');
    await v.waitForURL(`http://${domain}:8080/jobs`);
    await v.locator('h2:has-text("Open positions")').waitFor();
    await v.locator(`[data-cookie-banner]:has-text("Cookies ${s}?")`).waitFor();
    await v.click('[data-cookie-banner] button:has-text("Accept")');
    if (await v.locator('[data-cookie-banner]').count()) throw new Error('banner still shown');
    await v.screenshot({ path: '/tmp/nerp-e2e/site-own-domain.png' });
  });

  await step('set up delivery and a coupon', async () => {
    await ada.goto(B + '/crm/shop');
    await ada.fill('input[aria-label="Way of delivery"]', `Courier ${s}`);
    await ada.fill('input[aria-label="Delivery price"]', '4');
    await ada.click('[aria-label="Delivery"] button:has-text("Add")');
    await ada.locator(`[data-way-row="Courier ${s}"]`).waitFor();
    await ada.fill('input[aria-label="Coupon code"]', `ui${s}`);
    await ada.fill('input[aria-label="Discount"]', '25');
    await ada.click('button:has-text("Add coupon")');
    await ada.locator(`[data-coupon-row="UI${s.toUpperCase()}"]`).waitFor();
  });

  await step('a customer uses the coupon and chooses delivery', async () => {
    const c = await ctx(false);
    await c.goto(`${B}/shop/p/${slug}`);
    await c.click('button:has-text("Add to cart")');
    await c.goto(B + '/shop/cart');
    await c.fill('#co-name', `Cara ${s}`);
    await c.fill('#co-email', `cara.${s}@example.com`);
    await c.check(`[data-way="Courier ${s}"] input`);
    await c.fill('#co-addr', 'High St 2');
    await c.fill('#co-coupon', 'nope');
    await c.locator('[data-coupon-error]').waitFor();
    await c.fill('#co-coupon', `ui${s}`);
    await c.locator('[data-discount]:has-text("25%")').waitFor();
    await c.locator('[data-delivery-cost]').waitFor();
    const total = (await c.locator('[data-cart-total]').textContent())!;
    if (!total.includes('19')) throw new Error(`total ${total}`);
    await c.screenshot({ path: '/tmp/nerp-e2e/cart-coupon.png', fullPage: true });
    await c.click('button:has-text("Place order")');
    await c.waitForURL(/\/shop\/order\//);
  });

  await step('no browser errors', async () => {
    if (errors.length) throw new Error(errors.join(' | '));
  });
} finally {
  await browser.close();
  for (const cp of (await admin.get('/shop/coupons')).data.items ?? []) if (cp.code.includes(s.toUpperCase())) await admin.del(`/shop/coupons/${cp.id}`);
  for (const w of (await admin.get('/shop/delivery')).data.items ?? []) if (w.name.includes(s)) await admin.del(`/shop/delivery/${w.id}`);
  if (siteId) await admin.del(`/sites/${siteId}`);
  await admin.patch('/shop/settings', shopBefore);
  await cleanupRun(s);
}
console.log(failed ? `\n${failed} failed` : '\nall passed');
process.exit(failed ? 1 : 0);
