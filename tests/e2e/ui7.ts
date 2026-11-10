// Requisites UI: country fields, live checks, bank accounts, Bill to on deals.
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
const step = async (name, fn) => { try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); await page.screenshot({ path: `fail7-${++n}.png`, fullPage: true }); } };
const s = Date.now().toString(36);

const api = async (method: string, path: string, body?: any, token?: string | null) => (await fetch(B + '/api' + path, { method, headers: { 'content-type': 'application/json', ...(token ? { authorization: `Bearer ${token}` } : {}) }, body: body ? JSON.stringify(body) : undefined })).json();
const token = (await api('POST', '/auth/login', { email: 'admin@acme.test', password: 'supersecret1' })).access_token;
const co = await api('POST', '/crm/company/items', { company_name: `Infosys ${s}`, address: { city: 'Bengaluru', country: 'IN' } }, token);
const deal = await api('POST', '/crm/deal/items', { title: `Req UI deal ${s}`, company_id: co.id, currency: 'INR' }, token);

await page.goto(B + '/login'); await page.fill('#email', 'admin@acme.test'); await page.fill('#password', 'supersecret1');
await page.click('button:has-text("Sign in")'); await page.waitForURL(B + '/');

await step('add Indian requisites with live checks', async () => {
  await page.goto(B + `/crm/company/${co.id}`);
  await page.waitForSelector('h2:text-is("Requisites")');
  await page.click('section:has(h2:text-is("Requisites")) >> button:has-text("Add")');
  await page.waitForSelector('#rq-gstin');
  if ((await page.inputValue('#rq-country')) !== 'IN') throw new Error('country not prefilled from address');
  if ((await page.inputValue('#rq-name')) !== `Infosys ${s}`) throw new Error('name not prefilled');
  await page.fill('#rq-name', `Infosys Ltd ${s}`);
  await page.fill('#rq-gstin', '27AAPFU0939F1ZX');
  await page.click('#rq-pan');
  await page.waitForSelector('text=GSTIN is not valid');
  await page.fill('#rq-gstin', '27AAPFU0939F1ZV');
  await page.click('#rq-pan');
  await page.waitForSelector('text=GSTIN is not valid', { state: 'detached' });
  await page.fill('#rq-pan', 'AAPFU0939F');
  await page.click('button:has-text("Add account")');
  await page.fill('input[aria-label="Bank"]', 'HDFC Bank');
  await page.fill('input[aria-label="IFSC"]', 'HDFC0000123');
  await page.fill('input[aria-label="Account number"]', '50100012345678');
  await page.screenshot({ path: '13-requisite-form.png' });
  await page.click('dialog >> button:has-text("Save")');
  await page.waitForSelector(`text=Infosys Ltd ${s}`);
  await page.waitForSelector('dd:has-text("27AAPFU0939F1ZV")');
  await page.waitForSelector('dd:has-text("IFSC HDFC0000123")');
});

await step('switching country swaps the fields', async () => {
  await page.click('section:has(h2:text-is("Requisites")) >> button:has-text("Add")');
  await page.selectOption('#rq-country', 'DE');
  await page.waitForSelector('#rq-vat');
  if (await page.locator('#rq-gstin').count()) throw new Error('GSTIN still shown');
  await page.fill('#rq-name', `Infosys GmbH ${s}`);
  await page.fill('#rq-vat', 'DE123456789');
  await page.click('dialog >> button:has-text("Save")');
  await page.waitForSelector(`text=Infosys GmbH ${s}`);
  await page.screenshot({ path: '14-requisites.png', fullPage: true });
});

await step('Bill to on the deal + invoice uses it', async () => {
  await page.goto(B + `/crm/deal/${deal.id}`);
  await page.waitForSelector('#doc-buyer');
  const opts = await page.locator('#doc-buyer option').allTextContents();
  if (!opts[0].includes(`Infosys Ltd ${s}`) || opts.length !== 3) throw new Error('options: ' + opts.join(' | '));
  await page.selectOption('#doc-buyer', { label: opts.find((o) => o.includes('GmbH')) });
  await page.waitForTimeout(500);
  const buyer = await api('GET', `/crm/items/${deal.id}/buyer`, null, token);
  if (!buyer.options.find((o) => o.id === buyer.chosen)?.name.includes('GmbH')) throw new Error('not saved');
});

await step('timeline shows requisites', async () => {
  await page.goto(B + `/crm/company/${co.id}`);
  await page.waitForSelector(`text=added requisites “Infosys GmbH ${s}”`);
});

console.log(errors.length ? 'CONSOLE ERRORS:\n' + errors.join('\n') : 'no console errors');
await browser.close();
await cleanupRun(s);
process.exit(failed || errors.length ? 1 : 0);
