// Documents UI: seller settings, template editor with live preview, generating from a deal.
import { chromium } from 'playwright';
import { cleanupRun } from './cleanup.ts';
const B = 'http://localhost:8080';
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: any[] = [];
const ctx = await browser.newContext({ viewport: { width: 1440, height: 1000 } });
const page = await ctx.newPage();
page.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
page.on('console', (m) => m.type() === 'error' && !/401|400|Failed to load resource/.test(m.text()) && errors.push(m.text()));
let n = 0, failed = 0;
const step = async (name, fn) => { try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); await page.screenshot({ path: `fail6-${++n}.png`, fullPage: true }); } };
const s = Date.now().toString(36);

// A deal with products, created through the API.
const api = async (method: string, path: string, body?: any, token?: string | null) => (await fetch(B + '/api' + path, { method, headers: { 'content-type': 'application/json', ...(token ? { authorization: `Bearer ${token}` } : {}) }, body: body ? JSON.stringify(body) : undefined })).json();
const token = (await api('POST', '/auth/login', { email: 'admin@acme.test', password: 'supersecret1' })).access_token;
const co = await api('POST', '/crm/company/items', { company_name: `Tata ${s}` }, token);
const deal = await api('POST', '/crm/deal/items', { title: `Docs deal ${s}`, currency: 'INR', company_id: co.id }, token);
await api('PUT', `/crm/items/${deal.id}/products`, { rows: [{ name: 'Consulting', price: '123456.5', quantity: '1', unit: 'h', discount_kind: 'percent', discount_value: '0', tax_rate: '18', tax_included: false }] }, token);

await page.goto(B + '/login'); await page.fill('#email', 'admin@acme.test'); await page.fill('#password', 'supersecret1');
await page.click('button:has-text("Sign in")'); await page.waitForURL(B + '/');

await step('seller details in settings', async () => {
  await page.goto(B + '/settings');
  await page.fill('#s-legal', `Acme Pvt Ltd ${s}`);
  await page.fill('#s-taxid', '29ABCDE1234F1Z5');
  await page.fill('#s-bank', 'HDFC Bank · A/C 0012345678 · IFSC HDFC0000123');
  await page.click('button:has-text("Save settings")');
  await page.waitForSelector('text=Settings saved');
  await page.screenshot({ path: '10-seller.png', fullPage: true });
});

await step('documents tab lists built-ins; preview renders', async () => {
  await page.goto(B + '/crm/settings');
  await page.click('a[href="?tab=documents"]');
  await page.click('aside >> button:has-text("Invoice (LaTeX)")');
  await page.waitForSelector('iframe[title="PDF preview"]', { timeout: 30000 });
  await page.click('aside >> button:has-text("Invoice (Word)")');
  await page.waitForSelector('text=How to write a Word template');
  await page.click('button:has-text("Preview")');
  await page.waitForSelector('iframe[title="PDF preview"]', { timeout: 30000 });
  await page.click('aside >> button:has-text("Quote")');
  await page.waitForSelector('iframe[title="PDF preview"]');
  await page.screenshot({ path: '11-editor.png' });
});

await step('new Typst template: field insert, live error, save', async () => {
  await page.click('aside section:has-text("New template") >> button:has-text("Typst")');
  await page.fill('#dt-name', `Proforma ${s}`);
  await page.fill('#dt-title', 'Proforma invoice');
  await page.fill('#dt-prefix', `PF${s}-`);
  await page.fill('#dt-locale', 'en-IN');
  const ed = page.locator('textarea[aria-label="Template source"]');
  await ed.fill('#let d = json("data.json")\n= #d.doc.title\n#(');
  await page.waitForSelector('[role=alert]:has-text("line 3")', { timeout: 10000 });
  await ed.fill('#let d = json("data.json")\n= #d.doc.title\nFor ');
  await ed.press('End');
  await page.click('button[title="#d.client.name"]');
  if (!(await ed.inputValue()).includes('For #d.client.name')) throw new Error('field not inserted: ' + (await ed.inputValue()));
  await page.waitForFunction(() => !document.querySelector('[role=alert]'), null, { timeout: 10000 });
  await page.click('button:has-text("Create template")');
  await page.waitForSelector('text=Template saved');
  await page.waitForSelector(`aside >> text=Proforma ${s}`);
});

await step('generate Typst + LaTeX + Word documents from a deal', async () => {
  await page.goto(B + `/crm/deal/${deal.id}`);
  await page.waitForSelector('h2:text-is("Documents")');
  const popups: any[] = [];
  ctx.on('page', (p) => popups.push(p));
  await page.click('button:has-text("Create document")');
  await page.click(`[role=menu] >> button:has-text("Proforma ${s}")`);
  await page.waitForSelector(`text=PF${s}-0001`);
  for (const name of ['Invoice (LaTeX)', 'Invoice (Word)']) {
    await page.click('button:has-text("Create document")');
    await page.click(`[role=menu] >> button:has-text("${name}")`);
  }
  // External engines finish in the background; the list updates live.
  await page.waitForFunction(() => document.querySelectorAll('section:has(h2) li .badge').length === 0 && document.body.innerText.includes('DOCX'), null, { timeout: 60000 }).catch(() => {});
  await page.waitForSelector('text=Preparing…', { state: 'detached', timeout: 60000 });
  await page.waitForSelector('button:text-is("DOCX")');
  if (await page.locator('text=Failed').count()) throw new Error('a document failed');
  await page.waitForSelector('text=from “Invoice (LaTeX)”');
  await page.screenshot({ path: '12-deal-docs.png', fullPage: true });
});

await step('system page lists engines', async () => {
  await page.goto(B + '/system');
  await page.waitForSelector('text=Document engines');
  await page.waitForSelector('text=LuaHBTeX');
  await page.waitForSelector('text=LibreOffice 2');
});

await step('cleanup template', async () => {
  await page.goto(B + '/crm/settings');
  await page.click('a[href="?tab=documents"]');
  await page.click(`aside >> button:has-text("Proforma ${s}")`);
  page.once('dialog', (d) => d.accept());
  await page.click('button:has-text("Delete template")');
  await page.waitForSelector(`aside >> text=Proforma ${s}`, { state: 'detached' });
});

console.log(errors.length ? 'CONSOLE ERRORS:\n' + errors.join('\n') : 'no console errors');
await browser.close();
await cleanupRun(s);
process.exit(failed || errors.length ? 1 : 0);
