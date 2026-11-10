// Record card customisation in the UI: sections on the record page, the editor, the form
// following the layout, personal vs shared, reset.
import { chromium } from 'playwright';
import { cleanupRun } from './cleanup.ts';
const B = 'http://localhost:8080';
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: any[] = [];
const page = await (await browser.newContext({ viewport: { width: 1440, height: 1000 } })).newPage();
page.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
page.on('console', (m) => m.type() === 'error' && !/401|400|Failed to load resource/.test(m.text()) && errors.push(m.text()));
page.on('dialog', (d) => d.accept());
let n = 0, failed = 0;
const step = async (name, fn) => { try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); await page.screenshot({ path: `fail17-${++n}.png`, fullPage: true }); } };
const s = Date.now().toString(36);
const api = async (method: string, path: string, body?: any, token?: string | null) => (await fetch(B + '/api' + path, { method, headers: { 'content-type': 'application/json', ...(token ? { authorization: `Bearer ${token}` } : {}) }, body: body ? JSON.stringify(body) : undefined })).json();
const token = (await api('POST', '/auth/login', { email: 'admin@acme.test', password: 'supersecret1' })).access_token;
await fetch(B + '/api/crm/deal/card-layout?scope=common', { method: 'DELETE', headers: { authorization: `Bearer ${token}` } });
await fetch(B + '/api/crm/deal/card-layout?scope=mine', { method: 'DELETE', headers: { authorization: `Bearer ${token}` } });
const deal = await api('POST', '/crm/deal/items', { title: `Card ${s}`, close_date: '2027-03-15', probability: 40, source: 'website' }, token);

await page.goto(B + '/login'); await page.fill('#email', 'admin@acme.test'); await page.fill('#password', 'supersecret1');
await page.click('button:has-text("Sign in")'); await page.waitForURL(B + '/');
const sec = (t) => page.locator(`section[data-card-section]:has(h2:text-is("${t}"))`);

await step('default sections on the record', async () => {
  await page.goto(B + `/crm/deal/${deal.id}`);
  await sec('About').locator('[data-field="probability"] >> text=40%').waitFor();
  await sec('About').locator('[data-field="source"] >> text=Website').waitFor();
  await sec('Comment').waitFor();
});

await step('customize: rename, new section, move, hide, hide empty, for everyone', async () => {
  await sec('About').locator('button:has-text("Customize")').click();
  const dlg = page.locator('dialog[open]');
  await dlg.locator('text=Customize the deal card').waitFor();
  await dlg.locator('input[aria-label="Section title"]').first().fill('Deal facts');
  await dlg.locator('button:has-text("Add section")').click();
  await dlg.locator('input[aria-label="Section title"]').last().fill('Origin');
  await dlg.locator('select[aria-label="Move Source to section"]').selectOption({ label: 'Origin' });
  await dlg.locator('select[aria-label="Move Source details to section"]').selectOption({ label: 'Origin' });
  await dlg.locator('button[aria-label="Hide Probability, %"]').click();
  await dlg.locator('text=Hidden fields').waitFor();
  await dlg.locator('button:has-text("Probability, %")').waitFor();
  await dlg.locator('button[aria-label="Move section up"]').last().click();
  await dlg.locator('label:has-text("leave out fields without a value") input').check();
  await dlg.locator('input[type=radio][value=common]').check();
  await dlg.locator('button:has-text("Save")').click();
  await page.waitForSelector('text=Card saved for everyone');
  await sec('Deal facts').locator('[data-field="close_date"]').waitFor();
  await sec('Origin').locator('[data-field="source"] >> text=Website').waitFor();
  if (await page.locator('[data-field="probability"]').count()) throw new Error('probability still shown');
  if (await page.locator('[data-field="source_details"]').count()) throw new Error('empty field shown');
  if (await sec('Comment').count()) throw new Error('empty comment section shown');
  const order = await page.locator('section[data-card-section] h2').allTextContents();
  if (order.slice(0, 2).join() !== 'Deal facts,Origin') throw new Error(order.join());
});

await step('the form follows the layout', async () => {
  await page.click('button:has-text("Edit")');
  await page.locator('[data-form-section] h3:text-is("Deal facts")').waitFor();
  await page.locator('[data-form-section] h3:text-is("Origin")').waitFor();
  if (await page.locator('#crm-prob').count()) throw new Error('hidden field in the form');
  await page.fill('#crm-sdet', 'Spring campaign');
  await page.fill('#crm-close', '2027-04-01');
  await page.click('form button:has-text("Save")');
  await sec('Origin').locator('[data-field="source_details"] >> text=Spring campaign').waitFor();
  const d = await api('GET', `/crm/deal/items/${deal.id}`, null, token);
  if (d.probability !== 40 || d.close_date !== '2027-04-01') throw new Error(JSON.stringify([d.probability, d.close_date]));
});

await step('a new deal form shows the first section, the rest on request', async () => {
  await page.goto(B + '/crm/deal/new');
  await page.locator('[data-form-section] h3:text-is("Deal facts")').waitFor();
  await page.click('button:has-text("More fields")');
  await page.locator('[data-form-section] h3:text-is("Origin")').waitFor();
});

await step('reset to default', async () => {
  await page.goto(B + `/crm/deal/${deal.id}`);
  await sec('Deal facts').locator('button:has-text("Customize")').click();
  await page.click('dialog[open] button:has-text("Reset to default")');
  await page.waitForSelector('text=Card reset');
  await sec('About').locator('[data-field="probability"]').waitFor();
});

await page.screenshot({ path: '36-card.png', fullPage: true });
await api('DELETE', `/crm/deal/items/${deal.id}`, null, token); await api('DELETE', `/crm/recycle-bin/${deal.id}`, null, token);
console.log(errors.length ? 'console errors:\n' + errors.join('\n') : 'no console errors');
await browser.close();
await cleanupRun(s);
process.exit(failed || errors.length ? 1 : 0);
