// Smart processes UI: create a type, sidebar entry, records on board and detail page.
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
const step = async (name, fn) => { try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); await page.screenshot({ path: `fail9-${++n}.png`, fullPage: true }); } };
const s = Date.now().toString(36);

const api = async (method: string, path: string, body?: any, token?: string | null) => (await fetch(B + '/api' + path, { method, headers: { 'content-type': 'application/json', ...(token ? { authorization: `Bearer ${token}` } : {}) }, body: body ? JSON.stringify(body) : undefined })).json();
const token = (await api('POST', '/auth/login', { email: 'admin@acme.test', password: 'supersecret1' })).access_token;
const co = await api('POST', '/crm/company/items', { company_name: `Shipco ${s}` }, token);
let code;

await page.goto(B + '/login'); await page.fill('#email', 'admin@acme.test'); await page.fill('#password', 'supersecret1');
await page.click('button:has-text("Sign in")'); await page.waitForURL(B + '/');

await step('create a smart process in settings', async () => {
  await page.goto(B + '/crm/settings');
  await page.click('a[href="?tab=types"]');
  await page.click('button:has-text("New smart process")');
  await page.fill('#ty-name', `Shipment ${s}`);
  await page.fill('#ty-plural', `Shipments ${s}`);
  await page.click('button[aria-label="Icon send"]');
  await page.check('text=Amount and products');
  await page.click('dialog >> button:text-is("Create")');
  await page.waitForSelector(`aside >> text=Shipments ${s}`);
  code = `shipments-${s}`;
  await page.screenshot({ path: '18-types.png' });
});

await step('sidebar → board → new record with company', async () => {
  await page.click(`aside >> text=Shipments ${s}`);
  await page.waitForURL(new RegExp(`/crm/${code}$`));
  await page.waitForSelector('section h3:text-is("In progress")');
  await page.click(`a:has-text("New shipment ${s.toLowerCase()}")`);
  await page.fill('#crm-title', `Container ${s}`);
  await page.fill('input[placeholder="Find a company…"]', `Shipco ${s}`);
  await page.click(`ul >> button:has-text("Shipco ${s}")`);
  if (await page.locator('#crm-prob').count()) throw new Error('probability shown');
  await page.click(`button:has-text("Create shipment ${s.toLowerCase()}")`);
  await page.waitForSelector(`h1:has-text("Container ${s}")`);
  await page.waitForSelector('h2:text-is("Products")');
  await page.waitForSelector('h2:text-is("Documents")');
  await page.screenshot({ path: '19-smart-record.png', fullPage: true });
});

await step('shows on the company with its type name', async () => {
  await page.goto(B + `/crm/company/${co.id}`);
  await page.waitForSelector(`li:has-text("Container ${s}") >> text=Shipment ${s}`);
});

await step('pipelines and automation offer the new type', async () => {
  await page.goto(B + '/crm/settings');
  await page.click('a[href="?tab=automation"]'); await page.locator('button[role=tab]:has-text("Rules")').waitFor();
  await page.selectOption('select[aria-label="Record type"]', code);
  await page.waitForSelector('section[aria-label="Stage In progress"]');
});

await step('cleanup', async () => {
  const items = await api('GET', `/crm/${code}/items`, null, token);
  for (const it of items.items) { await api('DELETE', `/crm/${code}/items/${it.id}`, null, token); await api('DELETE', `/crm/recycle-bin/${it.id}`, null, token); }
  const r = await api('DELETE', `/crm/types/${code}`, null, token);
  if (!r.ok) throw new Error(JSON.stringify(r));
});

console.log(errors.length ? 'CONSOLE ERRORS:\n' + errors.join('\n') : 'no console errors');
await browser.close();
await cleanupRun(s);
process.exit(failed || errors.length ? 1 : 0);
