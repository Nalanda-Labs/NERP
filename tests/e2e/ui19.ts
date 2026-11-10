// API & webhooks in the app: create, use and revoke an API key; set up a webhook, send a
// test, read the delivery log; browse the API reference.
import { chromium } from 'playwright';
import http from 'node:http';
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
const step = async (name, fn) => { try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); await page.screenshot({ path: `fail19-${++n}.png`, fullPage: true }); } };
const s = Date.now().toString(36);
const hits: any[] = [];
const server = http.createServer((req, res) => { let b = ''; req.on('data', (c) => (b += c)); req.on('end', () => { hits.push(JSON.parse(b || '{}')); res.end('ok'); }); });
await new Promise<void>((r) => server.listen(0, '127.0.0.1', () => r()));

await page.goto(B + '/login'); await page.fill('#email', 'admin@acme.test'); await page.fill('#password', 'supersecret1');
await page.click('button:has-text("Sign in")'); await page.waitForURL(B + '/');

let secret;
await step('create an API key; the secret shows once', async () => {
  await page.click('aside >> a:has-text("API & webhooks")');
  await page.waitForURL(/\/developers/);
  await page.click('button:has-text("New key")');
  await page.fill('#k-name', `Sync ${s}`);
  await page.check('label:has-text("crm —") >> input');
  await page.selectOption('#k-exp', '30');
  await page.click('dialog[open] button:has-text("Create key")');
  secret = (await page.locator('[data-secret]').textContent())!.trim();
  if (!secret.startsWith('nerp_')) throw new Error(secret);
  await page.locator('pre:has-text("curl")').waitFor();
  await page.click('dialog[open] button:has-text("Done")');
  await page.locator(`tr:has-text("Sync ${s}") >> text=crm`).first().waitFor();
});

await step('the key works, then revoke it', async () => {
  const r = await fetch(B + '/api/crm/deal/items?limit=1', { headers: { authorization: `Bearer ${secret}` } });
  if (r.status !== 200) throw new Error('key refused ' + r.status);
  await page.reload();
  await page.locator(`tr:has-text("Sync ${s}") >> text=never`).first().waitFor({ state: 'detached', timeout: 5000 }).catch(() => {});
  await page.click(`tr:has-text("Sync ${s}") >> button:has-text("Revoke")`);
  await page.waitForSelector('text=Key revoked');
  await page.locator(`tr:has-text("Sync ${s}") >> text=Revoked`).waitFor();
  const r2 = await fetch(B + '/api/crm/deal/items?limit=1', { headers: { authorization: `Bearer ${secret}` } });
  if (r2.status !== 401) throw new Error('revoked key still works ' + r2.status);
});

await step('webhook: create, test, delivery log, send again', async () => {
  await page.click('a[href="?tab=webhooks"]');
  await page.click('button:has-text("New webhook")');
  await page.fill('#w-name', `Hook ${s}`);
  await page.fill('#w-url', `http://127.0.0.1:${(server.address() as { port: number }).port}/in`);
  await page.check('label:has-text("task.created") >> input');
  await page.click('dialog[open] button:has-text("Save")');
  await page.waitForSelector('text=Webhook saved');
  const row = page.locator(`[data-hook]:has-text("Hook ${s}")`);
  await row.locator('text=crm.item.created, task.created').waitFor();
  await row.locator('button:has-text("Send test")').click();
  for (let i = 0; i < 40 && !hits.some((h) => h.event === 'ping'); i++) await page.waitForTimeout(250);
  if (!hits.some((h) => h.event === 'ping')) throw new Error('no ping');
  await row.locator('button:has-text("Deliveries")').click();
  const d = page.locator('[data-delivery]:has-text("ping")').first();
  await d.locator('text=Delivered').waitFor();
  await d.locator('button').first().click();
  await d.locator('pre:has-text("Test from NERP")').waitFor();
  await d.locator('button:has-text("Send again")').click();
  await page.waitForSelector('text=Sent again');
  for (let i = 0; i < 40 && hits.filter((h) => h.event === 'ping').length < 2; i++) await page.waitForTimeout(250);
  if (hits.filter((h) => h.event === 'ping').length < 2) throw new Error('not sent again');
  await page.click('dialog[open] button:has-text("Close")');
  await row.locator('button').first().click();
  await page.locator('dialog[open] [data-secret]').waitFor();
  await page.click('dialog[open] button:has-text("Cancel")');
});

await step('API reference', async () => {
  await page.click('a[href="?tab=reference"]');
  await page.locator('section[aria-label="CRM"]').waitFor();
  await page.locator('section[aria-label="Tasks"]').waitFor();
  await page.fill('input[aria-label="Filter endpoints"]', 'observers');
  const op = page.locator('[data-op="put /crm/items/{id}/observers"]');
  await op.locator('button').click();
  await op.locator('text=user_ids').waitFor();
  await op.locator('text=API key scope:').waitFor();
  await page.fill('input[aria-label="Filter endpoints"]', '');
  await page.locator('[data-op="hook crm.item.updated"]').waitFor();
  await page.screenshot({ path: '38-api-reference.png', fullPage: true });
});

server.close();
await cleanupRun(s);
console.log(errors.length ? 'console errors:\n' + errors.join('\n') : 'no console errors');
await browser.close();
process.exit(failed || errors.length ? 1 : 0);
