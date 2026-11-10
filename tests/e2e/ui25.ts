// Catalog in the browser: a variant property and a price type in Catalog settings, variants
// made from a product, a variant's own price, and choosing a variant in the shop.
import { chromium } from 'playwright';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
const B = 'http://localhost:8080';
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: string[] = [];
const ctx = await browser.newContext({ viewport: { width: 1440, height: 1000 } });
const page = await ctx.newPage();
const watch = (p: import('playwright').Page) => {
  p.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
  p.on('console', (m) => m.type() === 'error' && !/401|400|404|Failed to load resource/.test(m.text()) && errors.push(m.text()));
};
watch(page);
page.on('dialog', (d) => d.accept());
const s = Date.now().toString(36);
let n = 0, failed = 0;
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); await page.screenshot({ path: `fail25-${++n}.png`, fullPage: true }); }
};

const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const base = (await admin.get('/crm/config')).data.base_currency;
const shopBefore = (await admin.get('/shop/settings')).data.settings;
await admin.patch('/shop/settings', { enabled: true, currency: base, payment_instructions: 'Pay by transfer', terms: null });
await admin.post('/crm/products', { name: `Cap ${s}`, price: '12', currency: base, in_shop: true });

await page.goto(B + '/login'); await page.fill('#email', 'admin@acme.test'); await page.fill('#password', 'supersecret1');
await page.click('button:has-text("Sign in")'); await page.waitForURL(B + '/');

try {
  await step('catalog settings', async () => {
    await page.click('aside a:has-text("Products")');
    await page.click('button:has-text("Catalog settings")');
    await page.fill('input[aria-label="New property"]', `Fit ${s}`);
    await page.selectOption('dialog[open] select[aria-label="Kind"]', 'list');
    await page.fill('input[aria-label="Values"]', 'Small, Large');
    await page.check('dialog[open] form label:has-text("Makes variants") input');
    await page.click('dialog[open] form button:has-text("Add") >> nth=0');
    await page.locator(`[data-prop="Fit ${s}"]`).waitFor();
    await page.fill('input[aria-label="New price type"]', `Trade ${s}`);
    await page.click('dialog[open] form:has(input[aria-label="New price type"]) button:has-text("Add")');
    await page.locator(`[data-price-type="Trade ${s}"]`).waitFor();
    await page.click('dialog[open] button:has-text("Done")');
  });

  await step('make variants', async () => {
    await page.fill('input[placeholder="Search name or SKU"]', s);
    await page.locator(`tr:has-text("Cap ${s}") button[aria-label="Edit"]`).click();
    await page.click('dialog[open] button:has-text("Make variants")');
    await page.click('dialog[open] button[aria-pressed]:text-is("Small")');
    await page.click('dialog[open] button[aria-pressed]:text-is("Large")');
    await page.click('dialog[open] button:has-text("Make 2 variants")');
    await page.locator('[data-variant="Large"]').waitFor();
    await page.locator('[data-variant="Small"]').waitFor();
  });

  await step('a variant of its own', async () => {
    await page.click('[data-variant="Large"] button');
    await page.locator('dialog[open] >> text=A variant of').waitFor();
    await page.fill('#p-p', '15');
    await page.fill(`label:has-text("Trade ${s}") + input`, '11');
    await page.click('dialog[open] button:has-text("Save")');
    await page.locator(`[data-product-name="Cap ${s} (Large)"]`).waitFor();
    await page.locator(`tr:has([data-product-name="Cap ${s} (Large)"]) >> text=$15.00`).waitFor();
  });

  await step('choose a variant in the shop', async () => {
    const shopper = await ctx.newPage();
    watch(shopper);
    await shopper.goto(`${B}/shop?q=${s}`);
    const card = shopper.locator(`[data-product="Cap ${s}"]`);
    await card.locator('text=from').waitFor();
    await card.locator('a:has-text("Choose")').click();
    await shopper.locator('text=Choose fit').waitFor();
    await shopper.click('button[aria-pressed]:text-is("Large")');
    await shopper.locator('[data-price]').filter({ hasText: '15.00' }).waitFor();
    await shopper.click('button:has-text("Add to cart")');
    await shopper.locator('[data-cart-count="1"]').waitFor();
    await shopper.screenshot({ path: '44-variant.png', fullPage: true });
    await shopper.close();
  });
} finally {
  await admin.patch('/shop/settings', shopBefore);
  await cleanupRun(s);
}
console.log(errors.length ? 'console errors:\n' + errors.join('\n') : 'no console errors');
await browser.close();
process.exit(failed || errors.length ? 1 : 0);
