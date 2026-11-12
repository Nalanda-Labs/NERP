// Audit integrity and company export in the browser: verifying the audit trail from the audit
// log, checking a noted head, and exporting everything from the Backups page.
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
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `${OUTPUT}/fail81-${n}-${i}.png`, fullPage: true }).catch(() => {}); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const before = new Set(((await admin.get('/admin/exports')).data.items as any[]).map((x) => x.id));

const ctx = await browser.newContext({ viewport: { width: 1400, height: 950 }, timezoneId: 'UTC', locale: 'en-GB', acceptDownloads: true });
const pa = await ctx.newPage();
pa.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
pa.on('console', (m) => m.type() === 'error' && !/400|401|403|404|Failed to load resource/.test(m.text()) && errors.push(m.text()));
pages.push(pa);
await pa.goto(B + '/login'); await pa.fill('#email', 'admin@acme.test'); await pa.fill('#password', 'supersecret1');
await pa.click('button:has-text("Sign in")'); await pa.waitForURL(B + '/');

try {
  await step('verifying the audit trail', async () => {
    await pa.goto(`${B}/audit`);
    const box = pa.locator('section[aria-label="Integrity"]');
    await box.locator('button:has-text("Verify now")').click();
    await box.locator('[data-integrity="ok"]').waitFor();
  });

  await step('checking a noted head', async () => {
    const box = pa.locator('section[aria-label="Integrity"]');
    const pos = (await box.locator('[data-head-position]').innerText()).replace(/\D/g, '');
    const hash = await box.locator('[data-head-hash]').innerText();
    await box.locator('summary').click();
    await box.locator('[aria-label="Noted record"]').fill(pos);
    await box.locator('[aria-label="Noted hash"]').fill(hash);
    await box.getByRole('button', { name: 'Check', exact: true }).click();
    await box.locator('[data-noted="ok"]').waitFor();
    await box.locator('[aria-label="Noted hash"]').fill('0'.repeat(64));
    await box.getByRole('button', { name: 'Check', exact: true }).click();
    await box.locator('[data-noted="bad"]').waitFor();
    await box.locator('[data-integrity="broken"]').waitFor();
    // Back to a clean result for whoever looks next.
    await box.locator('[aria-label="Noted record"]').fill('');
    await box.locator('button:has-text("Verify now")').click();
    await box.locator('[data-integrity="ok"]').waitFor();
  });

  await step('exporting everything', async () => {
    await pa.goto(`${B}/system/backups`);
    const box = pa.locator('section[aria-label="Company export"]');
    await box.locator('[data-export-files]').uncheck();
    await box.locator('[data-export-start]').click();
    await box.locator('[data-export="done"]').first().waitFor({ timeout: 120000 });
    const [dl] = await Promise.all([pa.waitForEvent('download'), box.locator('[data-export="done"]').first().locator('button:has-text("Download")').click()]);
    if (!/^nerp-export-.*\.zip$/.test(dl.suggestedFilename())) throw new Error(dl.suggestedFilename());
  });

  await step('no browser errors', async () => { if (errors.length) throw new Error(errors.join(' | ')); });
} finally {
  await browser.close();
  for (const x of (await admin.get('/admin/exports')).data.items as any[]) if (!before.has(x.id) && x.status !== 'running') await admin.req('DELETE', `/admin/exports/${x.id}`);
  await cleanupRun(s);
}
console.log(failed ? `\n${failed} failed` : '\nall passed');
process.exit(failed ? 1 : 0);
