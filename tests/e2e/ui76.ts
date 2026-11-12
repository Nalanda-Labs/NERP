// Stock value in the browser: choosing the cost method, the stock's value per product, cost
// of goods written off for a period, and costs in a product's movement history.
import { chromium } from 'playwright';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { OUTPUT } from './output.ts';
const B = 'http://localhost:8080';
const s = Date.now().toString(36);
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: string[] = [];
let failed = 0, n = 0;
const pages: import('playwright').Page[] = [];
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `${OUTPUT}/fail76-${n}-${i}.png`, fullPage: true }).catch(() => {}); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const before = (await admin.get('/inventory/settings')).data.settings;
const base = (await admin.get('/crm/config')).data.base_currency;
const wh = (await admin.post('/inventory/warehouses', { name: `Depot ${s}` })).data.items.find((w: any) => w.name === `Depot ${s}`);
const p = (await admin.post('/crm/products', { name: `Gizmo ${s}`, price: '30', currency: base })).data;
for (const [q, price] of [[4, 10], [4, 14]]) {
  const d = (await admin.post('/inventory/documents', { kind: 'receipt', to_warehouse: wh.id, comment: `ui ${s}`, items: [{ product_id: p.id, quantity: q, price }] })).data;
  await admin.post(`/inventory/documents/${d.id}/post`, {});
}
const wo = (await admin.post('/inventory/documents', { kind: 'writeoff', from_warehouse: wh.id, comment: `ui ${s}`, items: [{ product_id: p.id, quantity: 2 }] })).data;
await admin.post(`/inventory/documents/${wo.id}/post`, {});

const ctx = await browser.newContext({ viewport: { width: 1300, height: 900 }, timezoneId: 'UTC', locale: 'en-GB' });
const pa = await ctx.newPage();
pa.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
pa.on('console', (m) => m.type() === 'error' && !/400|401|403|404|Failed to load resource/.test(m.text()) && errors.push(m.text()));
pages.push(pa);
await pa.goto(B + '/login'); await pa.fill('#email', 'admin@acme.test'); await pa.fill('#password', 'supersecret1');
await pa.click('button:has-text("Sign in")'); await pa.waitForURL(B + '/');

try {
  await step('the value page: average cost', async () => {
    await admin.put('/inventory/settings', { cost_method: 'average', post_to_books: false });
    await pa.goto(B + '/inventory/value');
    await pa.locator('[data-costing]').waitFor();
    await pa.locator('[data-valuation] select').selectOption(wh.id);
    const row = pa.locator(`[data-valuation] [data-product="Gizmo ${s}"]`);
    await row.waitFor();
    // 8 at an average of 12, 2 written off: 6 × 12 = 72.
    if ((await row.locator('[data-value]').innerText()).replace(/[^\d.]/g, '') !== '72.00') throw new Error(await row.innerText());
  });

  await step('switching to FIFO revalues it', async () => {
    await pa.locator('[data-method="fifo"]').check();
    await pa.locator('[data-costing] button:has-text("Save")').click();
    await pa.locator('text=Saved').first().waitFor();
    // The oldest layer lost 2: 2 × 10 + 4 × 14 = 76.
    const row = pa.locator(`[data-valuation] [data-product="Gizmo ${s}"]`);
    await pa.waitForFunction((sel) => (document.querySelector(sel)?.textContent ?? '').includes('76.00'), `[data-valuation] [data-product="Gizmo ${s}"] [data-value]`, { timeout: 10000 });
    if ((await row.locator('[data-value]').innerText()).replace(/[^\d.]/g, '') !== '76.00') throw new Error('not revalued');
  });

  await step('the period\'s write-offs', async () => {
    const card = pa.locator('[data-cogs] [data-cogs-kind="written_off"]');
    await card.waitFor();
    if (Number((await card.innerText()).replace(/[^\d.]/g, '')) < 24) throw new Error(await card.innerText());
  });

  await step('costs in the movement history', async () => {
    await pa.goto(B + '/inventory');
    await pa.locator(`text=Gizmo ${s}`).first().click();
    const cell = pa.locator('[data-movement="document"] [data-cost]').first();
    await cell.waitFor();
    if (!/-?\d/.test(await cell.innerText())) throw new Error('no cost shown');
  });

  await step('no browser errors', async () => { if (errors.length) throw new Error(errors.join(' | ')); });
} finally {
  await browser.close();
  await admin.put('/inventory/settings', { cost_method: before.cost_method, post_to_books: before.post_to_books ?? false }).catch(() => {});
  await cleanupRun(s);
}
console.log(failed ? `\n${failed} failed` : '\nall passed');
process.exit(failed ? 1 : 0);
