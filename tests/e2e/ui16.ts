// Quotes and invoices in the UI: sidebar, making them from a deal, the deal's list with
// totals, due dates and overdue marks, and the record-type settings.
import { chromium } from 'playwright';
import { cleanupRun } from './cleanup.ts';
const B = 'http://localhost:8080';
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: any[] = [];
const page = await (await browser.newContext({ viewport: { width: 1440, height: 1000 } })).newPage();
page.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
page.on('console', (m) => m.type() === 'error' && !/401|400|Failed to load resource/.test(m.text()) && errors.push(m.text()));
let n = 0, failed = 0;
const step = async (name, fn) => { try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); await page.screenshot({ path: `fail16-${++n}.png`, fullPage: true }); } };
const s = Date.now().toString(36);
const api = async (method: string, path: string, body?: any, token?: string | null) => (await fetch(B + '/api' + path, { method, headers: { 'content-type': 'application/json', ...(token ? { authorization: `Bearer ${token}` } : {}) }, body: body ? JSON.stringify(body) : undefined })).json();
const token = (await api('POST', '/auth/login', { email: 'admin@acme.test', password: 'supersecret1' })).access_token;
const cfg = await api('GET', '/crm/config', null, token);
const made: any[] = [];
const deal = await api('POST', '/crm/deal/items', { title: `Office move ${s}`, currency: cfg.base_currency, opportunity: '2500' }, token); made.push(['deal', deal.id]);
let quoteUrl, invoiceUrl;

await page.goto(B + '/login'); await page.fill('#email', 'admin@acme.test'); await page.fill('#password', 'supersecret1');
await page.click('button:has-text("Sign in")'); await page.waitForURL(B + '/');

await step('quotes and invoices in the sidebar', async () => {
  await page.locator('aside >> a:has-text("Quotes")').waitFor();
  await page.locator('aside a[href="/crm/invoice"]').waitFor();
});

await step('create a quote from the deal', async () => {
  await page.goto(B + `/crm/deal/${deal.id}`);
  await page.click('button:has-text("Create quote")');
  await page.waitForURL(/\/crm\/quote\//);
  quoteUrl = page.url(); made.push(['quote', quoteUrl.split('/').pop()]);
  await page.locator(`h1:text-is("Office move ${s}")`).waitFor();
  await page.locator(`text=Created from`).waitFor();
  await page.locator('dt:text-is("Valid until")').waitFor();
  await page.locator('text=Draft').first().waitFor();
});

await step('create an invoice from the quote', async () => {
  if (await page.locator('button:has-text("Create quote")').count()) throw new Error('a quote offers another quote');
  await page.click('button:has-text("Create invoice")');
  await page.waitForURL(/\/crm\/invoice\//);
  invoiceUrl = page.url(); made.push(['invoice', invoiceUrl.split('/').pop()]);
  await page.locator('dt:text-is("Due date")').waitFor();
  await page.click('button:has-text("Edit")');
  await page.locator('label:text-is("Due date")').waitFor();
  const past = new Date(Date.now() - 4 * 86400000).toISOString().slice(0, 10);
  await page.fill('#crm-close', past);
  await page.click('button:has-text("Save")');
  await page.locator('dd >> text=Overdue').waitFor();
});

await step('deal lists both, with totals', async () => {
  await page.goto(B + `/crm/deal/${deal.id}`);
  const card = page.locator('section[aria-labelledby="made-h"]');
  await card.locator('h2:text-is("Quotes and invoices")').waitFor();
  await card.locator('text=Quote #').waitFor();
  await card.locator('text=Invoice #').waitFor();
  await card.locator('text=Overdue').waitFor();
  await card.locator('text=Invoiced').waitFor();
  const totals = (await card.locator('p:has-text("Invoiced")').textContent())!.replace(/\s+/g, ' ');
  if (!/Invoiced \S*2,500\.00 · paid \S*0\.00/.test(totals)) throw new Error(totals);
  await page.locator(`text=created quote #`).waitFor();
});

await step('pay the invoice: totals and overdue update', async () => {
  await page.goto(invoiceUrl);
  await page.click('button:has-text("Paid")');
  await page.locator('dt:text-is("Paid")').waitFor();
  if (await page.locator('dd >> text=Overdue').count()) throw new Error('paid invoice still overdue');
  await page.goto(B + `/crm/deal/${deal.id}`);
  const card = page.locator('section[aria-labelledby="made-h"]');
  await card.locator('li:has-text("Invoice #") >> text=Paid').waitFor();
  if (await card.locator('text=Overdue').count()) throw new Error('still overdue on the deal');
});

await step('overdue invoices on the board', async () => {
  const inv2 = await api('POST', `/crm/invoice/items/from/${deal.id}`, {}, token); made.push(['invoice', inv2.id]);
  await api('PATCH', `/crm/invoice/items/${inv2.id}`, { close_date: new Date(Date.now() - 2 * 86400000).toISOString().slice(0, 10) }, token);
  await page.goto(B + '/crm/invoice');
  await page.click('button[aria-label="Board"]');
  await page.locator(`a[href$="${inv2.id}"] >> text=Overdue since`).waitFor();
});

await step('record types: built in, not deletable', async () => {
  await page.goto(B + '/crm/settings?tab=types');
  const row = page.locator('main li:has-text("Invoices")').first();
  await row.locator('p.text-xs:has-text("/crm/invoice")').waitFor();
  if (!(await row.locator('p.text-xs').textContent())!.includes('Built in')) throw new Error('not marked built in');
  await row.locator('button:has-text("Edit")').click();
  await page.locator('dialog[open] >> text=Edit invoices').waitFor();
  if (await page.locator('dialog[open] button:has-text("Delete")').count()) throw new Error('delete offered');
  await page.click('dialog[open] button:has-text("Cancel")');
});

await page.screenshot({ path: '35-quotes-invoices.png', fullPage: true });
for (const [code, id] of made.reverse()) { await api('DELETE', `/crm/${code}/items/${id}`, null, token); await api('DELETE', `/crm/recycle-bin/${id}`, null, token); }
console.log(errors.length ? 'console errors:\n' + errors.join('\n') : 'no console errors');
await browser.close();
await cleanupRun(s);
process.exit(failed || errors.length ? 1 : 0);
