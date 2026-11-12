// Deal health in the browser: a deal's health card (score, grade, reasons), the Insights page
// listing open deals by health and filtering by grade, and the silent clients tab.
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
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `${OUTPUT}/fail80-${n}-${i}.png`, fullPage: true }).catch(() => {}); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
// No contact, the close date passed, nothing planned: at risk at best.
const deal = (await admin.post('/crm/deal/items', { title: `Shaky ${s}`, close_date: new Date(Date.now() - 5 * 86400e3).toISOString().slice(0, 10) })).data;

const ctx = await browser.newContext({ viewport: { width: 1400, height: 950 }, timezoneId: 'UTC', locale: 'en-GB' });
const pa = await ctx.newPage();
pa.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
pa.on('console', (m) => m.type() === 'error' && !/400|401|403|404|Failed to load resource/.test(m.text()) && errors.push(m.text()));
pages.push(pa);
await pa.goto(B + '/login'); await pa.fill('#email', 'admin@acme.test'); await pa.fill('#password', 'supersecret1');
await pa.click('button:has-text("Sign in")'); await pa.waitForURL(B + '/');

try {
  await step('the deal\'s health card', async () => {
    await pa.goto(`${B}/crm/deal/${deal.id}`);
    const card = pa.locator('[data-health]');
    await card.waitFor();
    if (Number(await card.locator('[data-score]').innerText()) > 65) throw new Error('should not be healthy');
    await card.locator('text=No contact person on the deal').waitFor();
    await card.locator('text=The close date passed 5 days ago').waitFor();
  });

  await step('Insights: the deal among the less healthy', async () => {
    await pa.click('a[href="/crm/insights"]');
    await pa.waitForURL(/\/crm\/insights/);
    await pa.locator(`[data-deal="Shaky ${s}"]`).waitFor();
    await pa.click('[data-grade="healthy"]');
    await pa.waitForFunction((t) => !document.querySelector(`[data-deal="${t}"]`), `Shaky ${s}`, { timeout: 10000 });
  });

  await step('the silent clients tab', async () => {
    await pa.click('[data-tab="silent"]');
    await pa.locator('section[aria-label="Silent clients"]').waitFor();
    await pa.locator('[data-silent-tasks]').waitFor();
  });

  await step('no browser errors', async () => { if (errors.length) throw new Error(errors.join(' | ')); });
} finally {
  await browser.close();
  await cleanupRun(s);
}
console.log(failed ? `\n${failed} failed` : '\nall passed');
process.exit(failed ? 1 : 0);
