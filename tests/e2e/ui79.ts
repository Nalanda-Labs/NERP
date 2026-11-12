// Cash forecast in the browser: from the Accounting menu, the weeks with the balance, opening
// a week to see what makes it up, and a longer horizon.
import { chromium } from 'playwright';
import { OUTPUT } from './output.ts';
const B = 'http://localhost:8080';
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: string[] = [];
let failed = 0, n = 0;
const pages: import('playwright').Page[] = [];
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `${OUTPUT}/fail79-${n}-${i}.png`, fullPage: true }).catch(() => {}); }
};
const ctx = await browser.newContext({ viewport: { width: 1400, height: 950 }, timezoneId: 'UTC', locale: 'en-GB' });
const pa = await ctx.newPage();
pa.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
pa.on('console', (m) => m.type() === 'error' && !/400|401|403|404|Failed to load resource/.test(m.text()) && errors.push(m.text()));
pages.push(pa);
await pa.goto(B + '/login'); await pa.fill('#email', 'admin@acme.test'); await pa.fill('#password', 'supersecret1');
await pa.click('button:has-text("Sign in")'); await pa.waitForURL(B + '/');

try {
  await step('from the menu to the forecast', async () => {
    await pa.click('a[href="/accounting/forecast"]');
    await pa.waitForURL(/\/accounting\/forecast/);
    await pa.locator('[data-opening]').waitFor();
    if ((await pa.locator('[data-week]').count()) !== 13) throw new Error('not 13 weeks');
    await pa.locator('[data-shortfall]').waitFor();
  });

  await step('opening a week', async () => {
    await pa.locator('[data-week]').first().click();
    await pa.locator('[data-week-items]').waitFor();
  });

  await step('a longer horizon', async () => {
    await pa.locator('select[aria-label="Weeks"]').selectOption('26');
    await pa.waitForFunction(() => document.querySelectorAll('[data-week]').length === 26, null, { timeout: 10000 });
  });

  await step('no browser errors', async () => { if (errors.length) throw new Error(errors.join(' | ')); });
} finally {
  await browser.close();
}
console.log(failed ? `\n${failed} failed` : '\nall passed');
process.exit(failed ? 1 : 0);
