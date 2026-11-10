// Collapsible sidebar groups: collapse, remembered after a reload, the current page stays
// visible inside a collapsed group, expand again.
import { chromium } from 'playwright';
import { cleanupRun } from './cleanup.ts';
const B = 'http://localhost:8080';
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: string[] = [];
const page = await (await browser.newContext({ viewport: { width: 1440, height: 1000 } })).newPage();
page.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
page.on('console', (m) => m.type() === 'error' && !/401|400|Failed to load resource/.test(m.text()) && errors.push(m.text()));
let n = 0, failed = 0;
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); await page.screenshot({ path: `fail20-${++n}.png`, fullPage: true }); }
};
const s = Date.now().toString(36);
const group = (title: string) => page.locator(`aside button[aria-expanded]:has-text("${title}")`);
const link = (label: string) => page.locator(`aside a:has-text("${label}")`);

await page.goto(B + '/login'); await page.fill('#email', 'admin@acme.test'); await page.fill('#password', 'supersecret1');
await page.click('button:has-text("Sign in")'); await page.waitForURL(B + '/');
await page.evaluate(() => localStorage.removeItem('nav.collapsed'));
await page.reload();

await step('groups start expanded', async () => {
  if ((await group('CRM').getAttribute('aria-expanded')) !== 'true') throw new Error('CRM collapsed');
  await link('Deals').waitFor();
  await link('Audit log').waitFor();
});

await step('collapse a group', async () => {
  await group('CRM').click();
  if ((await group('CRM').getAttribute('aria-expanded')) !== 'false') throw new Error('still expanded');
  await link('Deals').waitFor({ state: 'detached' });
  await link('Companies').waitFor({ state: 'detached' });
  await link('Audit log').waitFor(); // other groups unaffected
});

await step('remembered after a reload', async () => {
  await page.reload();
  await group('CRM').waitFor();
  if ((await group('CRM').getAttribute('aria-expanded')) !== 'false') throw new Error('not remembered');
  if (await link('Deals').count()) throw new Error('Deals shown');
});

await step('the current page stays visible in a collapsed group', async () => {
  await page.goto(B + '/crm/company');
  await link('Companies').waitFor();
  if (await link('Deals').count()) throw new Error('other items shown');
  if (!(await link('Companies').getAttribute('class'))!.includes('bg-brand-100')) throw new Error('not highlighted');
});

await step('expand again', async () => {
  await group('CRM').click();
  await link('Deals').waitFor();
  await link('Leads').waitFor();
  await page.reload();
  await link('Deals').waitFor();
});

await page.evaluate(() => localStorage.removeItem('nav.collapsed'));
await cleanupRun(s);
console.log(errors.length ? 'console errors:\n' + errors.join('\n') : 'no console errors');
await browser.close();
process.exit(failed || errors.length ? 1 : 0);
