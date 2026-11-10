// HR in the browser: start, pause and end the work day from the top bar; the timesheet;
// request time off; HR approves it; it shows on the chart.
import { chromium } from 'playwright';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
const B = 'http://localhost:8080';
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: string[] = [];
const s = Date.now().toString(36);
let n = 0, failed = 0;
const pages: import('playwright').Page[] = [];
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `fail32-${n}-${i}.png`, fullPage: true }); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
await admin.post('/users', { email: `uma.${s}@acme.test`, password: 'password123', first_name: 'Uma', last_name: `Worker${s}` });
const login = async (email: string, password: string) => {
  const p = await (await browser.newContext({ viewport: { width: 1440, height: 1000 } })).newPage();
  p.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
  p.on('console', (m) => m.type() === 'error' && !/401|400|404|Failed to load resource/.test(m.text()) && errors.push(m.text()));
  p.on('dialog', (d) => d.accept(''));
  pages.push(p);
  await p.goto(B + '/login'); await p.fill('#email', email); await p.fill('#password', password);
  await p.click('button:has-text("Sign in")'); await p.waitForURL(B + '/');
  return p;
};
const uma = await login(`uma.${s}@acme.test`, 'password123');
const year = new Date().getFullYear() + 1;

try {
  await step('the work day from the top bar', async () => {
    await uma.click('[data-workday="none"]');
    await uma.locator('[data-workday="working"]').click();
    await uma.click('[role="menuitem"]:has-text("Take a break")');
    await uma.locator('[data-workday="paused"]').click();
    await uma.click('[role="menuitem"]:has-text("Back to work")');
    await uma.locator('[data-workday="working"]').click();
    await uma.click('[role="menuitem"]:has-text("End the day")');
    await uma.locator('[data-workday="ended"]').waitFor();
  });

  await step('the timesheet', async () => {
    await uma.click('aside a:has-text("Working time")');
    await uma.locator(`[data-day="${new Date().toISOString().slice(0, 10)}"]`).or(uma.locator('[data-day]')).first().waitFor();
  });

  await step('request time off', async () => {
    await uma.click('aside a:has-text("Time off")');
    await uma.click('button:has-text("Request time off")');
    await uma.fill('#rq-s', `${year}-06-08`);
    await uma.fill('#rq-e', `${year}-06-10`);
    await uma.fill('#rq-n', `Wedding ${s}`);
    await uma.click('dialog[open] button:has-text("Request")');
    await uma.locator('text=Requested: 3 working days').waitFor();
    await uma.click('[role="tab"]:has-text("Mine")');
    await uma.locator('[data-request="vacation"] >> text=Waiting').waitFor();
  });

  const ada = await login('admin@acme.test', 'supersecret1');
  await step('HR approves; the chart shows it', async () => {
    await ada.click('aside a:has-text("Time off")');
    await ada.click('[role="tab"]:has-text("To approve")');
    await ada.locator(`[data-pending="Uma Worker${s}"] button:has-text("Approve")`).click();
    await ada.locator(`[data-pending="Uma Worker${s}"]`).waitFor({ state: 'detached' });
    await uma.reload();
    await uma.click('[role="tab"]:has-text("Mine")');
    await uma.locator('[data-request="vacation"] >> text=Approved').waitFor();
    await uma.click('[role="tab"]:has-text("Who is away")');
    const now = new Date();
    for (let i = 0; i < (year - now.getFullYear()) * 12 + 5 - now.getMonth(); i++) await uma.click('button[aria-label="Next month"]');
    await uma.locator(`text=June ${year}`).waitFor();
    await uma.locator(`[data-chart-person="Uma Worker${s}"] [data-away="vacation"]`).first().waitFor();
  });
  await uma.screenshot({ path: '52-time-off.png' });
} finally {
  await cleanupRun(s);
}
console.log(errors.length ? 'console errors:\n' + errors.join('\n') : 'no console errors');
await browser.close();
process.exit(failed || errors.length ? 1 : 0);
