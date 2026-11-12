// System → Health in the browser: reached from System, the checks with their levels, the
// self-test, and the alert settings.
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
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `${OUTPUT}/fail82-${n}-${i}.png`, fullPage: true }).catch(() => {}); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const before = (await admin.get('/admin/health')).data.settings;

const ctx = await browser.newContext({ viewport: { width: 1400, height: 950 }, timezoneId: 'UTC', locale: 'en-GB' });
const pa = await ctx.newPage();
pa.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
pa.on('console', (m) => m.type() === 'error' && !/400|401|403|404|Failed to load resource/.test(m.text()) && errors.push(m.text()));
pages.push(pa);
await pa.goto(B + '/login'); await pa.fill('#email', 'admin@acme.test'); await pa.fill('#password', 'supersecret1');
await pa.click('button:has-text("Sign in")'); await pa.waitForURL(B + '/');

try {
  await step('from System to Health', async () => {
    await pa.goto(`${B}/system`);
    await pa.click('a[href="/system/health"]');
    await pa.waitForURL(/\/system\/health/);
    await pa.locator('[data-overall]').waitFor();
    await pa.locator('section[aria-label="Checks"] [data-check="database"][data-level="ok"]').waitFor();
    await pa.locator('[data-check="certificate"][data-level="off"]').waitFor();
  });

  await step('the self-test', async () => {
    await pa.click('[data-run-health]');
    const t = pa.locator('section[aria-label="Self-test"]');
    for (const k of ['test_database', 'test_redis', 'test_files']) await t.locator(`[data-check="${k}"][data-level="ok"]`).waitFor();
  });

  await step('alert settings', async () => {
    await pa.fill('[data-extra-emails]', `ops.${s}@example.com`);
    await pa.click('section[aria-label="Alerts"] button:has-text("Save")');
    await pa.locator('text=Saved').waitFor();
    if ((await admin.get('/admin/health')).data.settings.extra_emails[0] !== `ops.${s}@example.com`) throw new Error('not saved');
  });

  await step('no browser errors', async () => { if (errors.length) throw new Error(errors.join(' | ')); });
} finally {
  await browser.close();
  await admin.put('/admin/health/settings', before ?? { alerts: true, extra_emails: [] }).catch(() => {});
  await cleanupRun(s);
}
console.log(failed ? `\n${failed} failed` : '\nall passed');
process.exit(failed ? 1 : 0);
