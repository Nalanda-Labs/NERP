// True profit in the browser: from Sales analytics to the report, a client's row with its
// revenue, goods and profit, switching to products, and setting a person's hourly cost.
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
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `${OUTPUT}/fail78-${n}-${i}.png`, fullPage: true }).catch(() => {}); }
};
const admin = new Client();
const me = (await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' })).data.user;
const before = (await admin.get('/crm/profit/settings')).data;
const base = (await admin.get('/crm/config')).data.base_currency;
const acme = (await admin.post('/crm/company/items', { company_name: `Globex ${s}` })).data;
const gadget = (await admin.post('/crm/products', { name: `Gadget ${s}`, price: '50', currency: base, cost: '20' })).data;
const inv = (await admin.post('/crm/invoice/items', { title: `Invoice ${s}`, company_id: acme.id, currency: base })).data;
await admin.put(`/crm/items/${inv.id}/products`, { rows: [{ product_id: gadget.id, name: `Gadget ${s}`, price: '50', quantity: '3', unit: 'pcs', discount_kind: 'percent', discount_value: '0', tax_rate: null, tax_included: false }] });

const ctx = await browser.newContext({ viewport: { width: 1400, height: 950 }, timezoneId: 'UTC', locale: 'en-GB' });
const pa = await ctx.newPage();
pa.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
pa.on('console', (m) => m.type() === 'error' && !/400|401|403|404|Failed to load resource/.test(m.text()) && errors.push(m.text()));
pages.push(pa);
await pa.goto(B + '/login'); await pa.fill('#email', 'admin@acme.test'); await pa.fill('#password', 'supersecret1');
await pa.click('button:has-text("Sign in")'); await pa.waitForURL(B + '/');

try {
  await step('from Sales analytics to True profit', async () => {
    await pa.goto(B + '/crm/analytics');
    await pa.click('[data-true-profit]');
    await pa.waitForURL(/\/crm\/analytics\/profit/);
    await pa.locator('h1:has-text("True profit")').waitFor();
  });

  await step('the client\'s row: 150 revenue, 60 goods, 90 profit', async () => {
    const row = pa.locator(`[data-row="Globex ${s}"]`);
    await row.waitFor();
    const cell = async (k: string) => (await row.locator(`[data-col="${k}"]`).innerText()).replace(/[^\d.]/g, '');
    if (await cell('revenue') !== '150.00' || await cell('goods') !== '60.00' || await cell('profit') !== '90.00') throw new Error(await row.innerText());
  });

  await step('by product', async () => {
    await pa.click('[data-by="product"]');
    await pa.locator(`[data-row="Gadget ${s}"]`).waitFor();
  });

  await step('setting a person\'s hourly cost', async () => {
    await pa.click('[data-profit-settings]');
    const input = pa.locator(`input[aria-label="Hourly cost of ${me.first_name} ${me.last_name}"]`);
    await input.waitFor();
    await input.fill('45');
    await pa.click('button[form="pf-settings"]');
    await pa.locator('text=Saved').first().waitFor();
    const p = (await admin.get('/crm/profit/settings')).data.people.find((x: any) => x.id === me.id);
    if (Number(p?.hourly) !== 45) throw new Error(JSON.stringify(p));
  });

  await step('no browser errors', async () => { if (errors.length) throw new Error(errors.join(' | ')); });
} finally {
  await browser.close();
  await admin.put('/crm/profit/costs', [{ user_id: me.id, hourly: before.people?.find((x: any) => x.id === me.id)?.hourly ?? null }]).catch(() => {});
  await cleanupRun(s);
}
console.log(failed ? `\n${failed} failed` : '\nall passed');
process.exit(failed ? 1 : 0);
