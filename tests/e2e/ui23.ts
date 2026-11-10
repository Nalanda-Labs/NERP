// Recurring records in the browser: set a deal to repeat monthly, make a copy now, see it
// listed on the pattern, stop repeating.
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
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); await page.screenshot({ path: `fail23-${++n}.png`, fullPage: true }); }
};

const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const deal = (await admin.post('/crm/deal/items', { title: `Hosting ${s}` })).data;

await page.goto(B + '/login'); await page.fill('#email', 'admin@acme.test'); await page.fill('#password', 'supersecret1');
await page.click('button:has-text("Sign in")'); await page.waitForURL(B + '/');

try {
  await step('set it to repeat', async () => {
    await page.goto(`${B}/crm/deal/${deal.id}`);
    const card = page.locator('[data-recurring]');
    await card.locator("text=Doesn't repeat").waitFor();
    await card.locator('button:has-text("Set up")').click();
    await page.selectOption('#rec-freq', 'monthly');
    await page.fill('#rep-due', '10');
    await page.click('dialog[open] button:has-text("Save")');
    await card.locator('[data-summary]').filter({ hasText: 'Every month' }).waitFor();
    await card.locator('text=0 made on schedule').waitFor();
    await page.locator('text=set it to repeat').waitFor();
  });

  await step('create one now', async () => {
    await page.click('[data-recurring] button:has-text("Create one now")');
    await page.waitForURL((u) => u.pathname.startsWith('/crm/deal/') && !u.pathname.endsWith(deal.id));
    await page.locator('text=created this as a repeat').waitFor();
    await page.goto(`${B}/crm/deal/${deal.id}`);
    await page.locator(`section[aria-labelledby="made-h"] >> text=Hosting ${s}`).waitFor();
  });
  await page.screenshot({ path: '42-recurring.png', fullPage: true });

  await step('stop repeating', async () => {
    await page.click('[data-recurring] button:has-text("Stop repeating")');
    await page.locator("[data-recurring] >> text=Doesn't repeat").waitFor();
  });
} finally {
  await cleanupRun(s);
}
console.log(errors.length ? 'console errors:\n' + errors.join('\n') : 'no console errors');
await browser.close();
process.exit(failed || errors.length ? 1 : 0);
