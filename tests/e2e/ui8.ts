// Automation UI: robots and triggers per stage, live effect on a deal.
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
const step = async (name, fn) => { try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); await page.screenshot({ path: `fail8-${++n}.png`, fullPage: true }); } };
const s = Date.now().toString(36);

const api = async (method: string, path: string, body?: any, token?: string | null) => (await fetch(B + '/api' + path, { method, headers: { 'content-type': 'application/json', ...(token ? { authorization: `Bearer ${token}` } : {}) }, body: body ? JSON.stringify(body) : undefined })).json();
const token = (await api('POST', '/auth/login', { email: 'admin@acme.test', password: 'supersecret1' })).access_token;
const cats = await api('POST', '/crm/deal/categories', { name: `UI auto ${s}` }, token);
const cat = cats.categories.find((c) => c.name === `UI auto ${s}`);

await page.goto(B + '/login'); await page.fill('#email', 'admin@acme.test'); await page.fill('#password', 'supersecret1');
await page.click('button:has-text("Sign in")'); await page.waitForURL(B + '/');

await step('automation tab shows stage columns', async () => {
  await page.goto(B + '/crm/settings');
  await page.click('a[href="?tab=automation"]'); await page.locator('button[role=tab]:has-text("Rules")').waitFor();
  await page.selectOption('select[aria-label="Pipeline"]', cat.id);
  await page.waitForSelector('section[aria-label="Stage New"]');
  await page.waitForSelector('section[aria-label="Stage Won"]');
});

await step('add a comment robot with a placeholder and a delay-free run', async () => {
  await page.click('section[aria-label="Stage New"] >> button:has-text("+ Robot")');
  await page.selectOption('#au-action', 'comment');
  await page.fill('#au-name', 'Welcome note');
  await page.fill('#au-text', 'Hello ');
  await page.click('dialog >> button:has-text("{{title}}")');
  if ((await page.inputValue('#au-text')) !== 'Hello {{title}}') throw new Error('placeholder not inserted');
  await page.click('dialog >> button:has-text("Add condition")');
  await page.selectOption('select[aria-label="Field"]', 'opportunity');
  await page.selectOption('select[aria-label="Operator"]', 'gte');
  await page.fill('input[aria-label="Value"]', '0');
  await page.click('dialog >> button:text-is("Save")');
  await page.waitForSelector('section[aria-label="Stage New"] >> text=Welcome note');
});

await step('add an activity robot for a specific delay', async () => {
  await page.click('section[aria-label="Stage New"] >> button:has-text("+ Robot")');
  await page.fill('#au-subject', 'Call {{title}}');
  await page.selectOption('select[aria-label="When"]', 'delay');
  await page.fill('input[aria-label="Delay"]', '2');
  await page.selectOption('select[aria-label="Delay unit"]', '60');
  await page.click('dialog >> button:text-is("Save")');
  await page.waitForSelector('section[aria-label="Stage New"] >> text=after 2 h');
});

await step('webhook trigger shows its URL', async () => {
  await page.click('section[aria-label="Stage Won"] >> button:has-text("+ Trigger")');
  await page.selectOption('#au-event', 'webhook');
  await page.fill('#au-name', 'Paid in ERP');
  await page.click('dialog >> button:text-is("Save")');
  await page.click('section[aria-label="Stage Won"] >> button:has-text("Paid in ERP")');
  await page.waitForSelector('code:has-text("/api/crm/automation/webhook/")');
  await page.screenshot({ path: '15-automation-trigger.png' });
  await page.click('dialog >> button:text-is("Cancel")');
  await page.screenshot({ path: '16-automation.png' });
});

await step('a new deal runs the robot; waiting robot listed; webhook moves it', async () => {
  const deal = await api('POST', '/crm/deal/items', { title: `Auto UI ${s}`, category_id: cat.id }, token);
  await page.goto(B + `/crm/deal/${deal.id}`);
  await page.waitForSelector(`text=ran “Welcome note”: added a comment`, { timeout: 15000 });
  await page.waitForSelector(`text=Hello Auto UI ${s}`);
  await page.waitForSelector('h2:text-is("Automation waiting")');
  const overview = await api('GET', `/crm/automation?category_id=${cat.id}`, null, token);
  const hook = overview.triggers.find((t) => t.name === 'Paid in ERP').token;
  await fetch(B + `/api/crm/automation/webhook/${hook}`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ item_id: deal.id }) });
  await page.waitForSelector('text=moved it by the trigger “Paid in ERP”', { timeout: 10000 });
  await page.screenshot({ path: '17-deal-automation.png', fullPage: true });
  const defaultCat = (await api('GET', '/crm/config', null, token)).types.find((t) => t.code === 'deal').categories.find((c) => c.is_default).id;
  await api('PATCH', `/crm/deal/items/${deal.id}`, { category_id: defaultCat }, token);
  await api('DELETE', `/crm/deal/items/${deal.id}`, null, token);
  await api('DELETE', `/crm/categories/${cat.id}`, null, token);
});

console.log(errors.length ? 'CONSOLE ERRORS:\n' + errors.join('\n') : 'no console errors');
await browser.close();
await cleanupRun(s);
process.exit(failed || errors.length ? 1 : 0);
