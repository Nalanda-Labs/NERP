// The online shop in the browser: open the shop, set up a product (category, photo, stock),
// shop as a visitor (catalog, product page, cart, checkout, order page), then see the order
// in the CRM.
import { chromium } from 'playwright';
import { Client, waitFor } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
const B = 'http://localhost:8080';
const MP = 'http://localhost:8026/api/v1';
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: string[] = [];
const watch = (p: import('playwright').Page) => {
  p.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
  p.on('console', (m) => m.type() === 'error' && !/401|400|404|Failed to load resource/.test(m.text()) && errors.push(m.text()));
};
const page = await (await browser.newContext({ viewport: { width: 1440, height: 1000 } })).newPage();
watch(page);
const pages = [page];
const s = Date.now().toString(36);
page.on('dialog', (d) => d.accept());
let n = 0, failed = 0;
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `fail22-${n}-${i}.png`, fullPage: true }); }
};
const PNG = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==', 'base64');

const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const before = (await admin.get('/shop/settings')).data.settings;

await page.goto(B + '/login'); await page.fill('#email', 'admin@acme.test'); await page.fill('#password', 'supersecret1');
await page.click('button:has-text("Sign in")'); await page.waitForURL(B + '/');

try {
  await step('open the shop', async () => {
    await page.click('aside a:has-text("Online shop")');
    await page.fill('#sh-name', `Corner Store ${s}`);
    await page.fill('#sh-pay', 'Pay {{total}} to account 123, reference {{order}}.');
    await page.fill('#sh-terms', '');
    await page.click('button:has-text("Save")');
    await page.waitForSelector('text=Saved');
    await page.click('button:has-text("Open the shop")');
    await page.locator('[data-shop-status] >> text=Open').waitFor();
  });

  await step('a product for the shop', async () => {
    await page.click('aside a:has-text("Products")');
    await page.click('button:has-text("Shop categories")');
    await page.fill('input[aria-label="New category"]', `Lamps ${s}`);
    await page.click('dialog[open] button:has-text("Add")');
    await page.locator(`[data-category="Lamps ${s}"]`).waitFor();
    await page.click('dialog[open] button:has-text("Done")');
    await page.click('button:has-text("New product")');
    await page.fill('#p-n', `Desk lamp ${s}`);
    await page.fill('#p-p', '40');
    await page.fill('#p-d', 'Warm light');
    await page.fill('#p-st', '2');
    await page.check('label:has-text("Sell in the online shop") input');
    await page.selectOption('#p-cat', { label: `Lamps ${s}` });
    await page.setInputFiles('input[aria-label="Product photo"]', { name: `lamp-${s}.png`, mimeType: 'image/png', buffer: PNG });
    await page.locator('img[data-photo]').waitFor();
    await page.click('dialog[open] button:has-text("Save")');
    await page.locator(`tr:has-text("Desk lamp ${s}") >> text=In the shop`).waitFor();
    await page.locator(`[data-stock="Desk lamp ${s}"]:text-is("2")`).waitFor();
  });

  const shopper = await (await browser.newContext({ viewport: { width: 390, height: 844 } })).newPage();
  watch(shopper);
  pages.push(shopper);
  await step('browse and add to the cart', async () => {
    await shopper.goto(`${B}/shop?q=${s}`);
    await shopper.locator(`h1:text-is("Corner Store ${s}")`).waitFor();
    await shopper.click(`button:has-text("Lamps ${s}")`);
    const card = shopper.locator(`[data-product="Desk lamp ${s}"]`);
    await card.locator('img').waitFor();
    await card.locator('a').last().click();
    await shopper.locator('[data-left]:text-is("Only 2 left")').waitFor();
    await shopper.fill('#qty', '2');
    await shopper.click('button:has-text("Add to cart")');
    await shopper.locator('[data-cart-count="2"]').waitFor();
    await shopper.locator('text=All 2 left are in your cart.').waitFor();
  });

  let token = '';
  await step('check out', async () => {
    await shopper.click('a:has-text("Go to your cart")');
    await shopper.locator(`[data-line="Desk lamp ${s}"]`).waitFor();
    await shopper.locator('[data-cart-total]').filter({ hasText: '80' }).waitFor();
    await shopper.fill('#co-name', `Lou Reed ${s}`);
    await shopper.fill('#co-email', `lou.${s}@example.com`);
    await shopper.fill('#co-addr', '5 Elm Road');
    await shopper.click('button:has-text("Place order")');
    await shopper.waitForURL(/\/shop\/order\/[\w-]+\?placed=1/);
    token = shopper.url().split('/order/')[1].split('?')[0];
    await shopper.locator('text=Thank you! Your order is placed.').waitFor();
    await shopper.locator('[data-order-stage="New"]').waitFor();
    await shopper.locator('[data-pay]').filter({ hasText: 'account 123' }).waitFor();
    await shopper.locator('[data-cart-count="0"]').waitFor();
    const m = await waitFor(async () => (await (await fetch(`${MP}/search?query=${encodeURIComponent(`to:lou.${s}@example.com`)}`)).json()).messages?.[0], 15000);
    if (!m) throw new Error('no confirmation email');
  });

  await step('sold out after the order', async () => {
    await shopper.goto(`${B}/shop?q=${s}`);
    await shopper.locator(`[data-product="Desk lamp ${s}"] >> text=Sold out`).waitFor();
  });
  await shopper.screenshot({ path: '40-shop-order.png', fullPage: true });

  await step('the order in the CRM', async () => {
    await page.click('aside a:has-text("Orders")');
    await page.click(`a:has-text("Lou Reed ${s}")`);
    const card = page.locator('[data-shop-order]');
    await card.locator('text=5 Elm Road').waitFor();
    await card.locator(`a:has-text("lou.${s}@example.com")`).waitFor();
    if (!(await card.locator('a:has-text("Customer\'s order page")').getAttribute('href'))?.endsWith(token)) throw new Error('order page link');
    await page.locator('text=placed this order').waitFor();
    await page.locator('button:has-text("Create invoice")').waitFor();
  });
  await page.screenshot({ path: '41-order-record.png', fullPage: true });
} finally {
  await admin.patch('/shop/settings', before);
  await cleanupRun(s);
}
console.log(errors.length ? 'console errors:\n' + errors.join('\n') : 'no console errors');
await browser.close();
process.exit(failed || errors.length ? 1 : 0);
