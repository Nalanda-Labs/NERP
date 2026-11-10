// Inventory in the browser: add a warehouse, receive goods, see stock and its history,
// transfer between warehouses, cancel a document.
import { chromium } from 'playwright';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
const B = 'http://localhost:8080';
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: string[] = [];
const page = await (await browser.newContext({ viewport: { width: 1440, height: 1000 } })).newPage();
page.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
page.on('console', (m) => m.type() === 'error' && !/401|400|404|Failed to load resource/.test(m.text()) && errors.push(m.text()));
page.on('dialog', (d) => d.accept());
const s = Date.now().toString(36);
let n = 0, failed = 0;
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); await page.screenshot({ path: `fail24-${++n}.png`, fullPage: true }); }
};

const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const base = (await admin.get('/crm/config')).data.base_currency;
await admin.post('/crm/products', { name: `Widget ${s}`, price: '9', currency: base });

await page.goto(B + '/login'); await page.fill('#email', 'admin@acme.test'); await page.fill('#password', 'supersecret1');
await page.click('button:has-text("Sign in")'); await page.waitForURL(B + '/');

try {
  await step('add a warehouse', async () => {
    await page.click('aside a:has-text("Warehouses")');
    await page.click('button:has-text("New warehouse")');
    await page.fill('#wh-name', `Depot ${s}`);
    await page.click('dialog[open] button:has-text("Save")');
    await page.locator(`[data-warehouse="Depot ${s}"]`).waitFor();
  });

  await step('receive goods', async () => {
    await page.click('aside a:has-text("Stock documents")');
    await page.click('button:has-text("Receive goods")');
    await page.waitForURL(/\/inventory\/documents\/[0-9a-f-]{36}/);
    await page.fill('#d-com', `Delivery ${s}`);
    await page.fill('input[aria-label="Add a product"]', `Widget ${s}`);
    await page.click(`button:has-text("Widget ${s}")`);
    await page.fill(`input[aria-label="Quantity of Widget ${s}"]`, '12');
    await page.fill(`input[aria-label="Price of Widget ${s}"]`, '2.5');
    await page.locator('[data-cost]').filter({ hasText: '30' }).waitFor();
    await page.click('button:has-text("Post")');
    await page.locator('[data-status="posted"]').waitFor();
  });
  const receipt = page.url();

  await step('stock and history', async () => {
    await page.click('aside a:has-text("Stock")');
    await page.fill('input[aria-label="Search name or SKU"]', s);
    const row = page.locator(`[data-stock-row="Widget ${s}"]`);
    await row.locator('[data-total]').filter({ hasText: '12' }).waitFor();
    await row.click();
    await page.locator('dialog[open] [data-movement="document"] >> text=RC-').waitFor();
    await page.click('dialog[open] button:has-text("Close")');
  });

  await step('transfer to the new warehouse', async () => {
    await page.click('button:has-text("Transfer")');
    await page.waitForURL(/\/inventory\/documents\/[0-9a-f-]{36}/);
    await page.selectOption('#d-to', { label: `Depot ${s}` });
    await page.fill('#d-com', `Move ${s}`);
    await page.fill('input[aria-label="Add a product"]', `Widget ${s}`);
    await page.click(`button:has-text("Widget ${s}")`);
    await page.fill(`input[aria-label="Quantity of Widget ${s}"]`, '5');
    await page.click('button:has-text("Post")');
    await page.locator('[data-status="posted"]').waitFor();
    await page.click('aside a:has-text("Stock")');
    await page.fill('input[aria-label="Search name or SKU"]', s);
    await page.locator(`[data-stock-row="Widget ${s}"] [data-wh="Depot ${s}"]:text-is("5")`).waitFor();
  });
  await page.screenshot({ path: '43-stock.png', fullPage: true });

  await step('a receipt whose goods moved on cannot be cancelled, a transfer can', async () => {
    await page.goto(receipt);
    await page.click('button:has-text("Cancel document")');
    await page.locator('text=left in').first().waitFor();
    await page.click('aside a:has-text("Stock documents")');
    await page.click(`[data-doc] a >> nth=0`);
    await page.click('button:has-text("Cancel document")');
    await page.locator('[data-status="cancelled"]').waitFor();
  });
} finally {
  await cleanupRun(s);
}
console.log(errors.length ? 'console errors:\n' + errors.join('\n') : 'no console errors');
await browser.close();
process.exit(failed || errors.length ? 1 : 0);
