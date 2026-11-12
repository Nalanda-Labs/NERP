// Automation preview in the browser: a new robot's editor says how often it would have run.
import { chromium } from 'playwright';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { OUTPUT } from './output.ts';
import { dbUrl } from '../db/cleanup.ts';
import pg from 'pg';
const B = 'http://localhost:8080';
const s = Date.now().toString(36);
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: string[] = [];
let failed = 0, n = 0;
const pages: import('playwright').Page[] = [];
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `${OUTPUT}/fail92-${n}-${i}.png`, fullPage: true }).catch(() => {}); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
const pipe = (await admin.post('/crm/deal/categories', { name: `UiPreview ${s}` })).data.categories.find((c: any) => c.name === `UiPreview ${s}`);
const first = pipe.stages[0];
for (let i = 0; i < 3; i++) await admin.post('/crm/deal/items', { title: `Pv ${i} ${s}`, category_id: pipe.id, stage_id: first.id });

const ctx = await browser.newContext({ viewport: { width: 1400, height: 950 }, timezoneId: 'UTC', locale: 'en-GB' });
const pa = await ctx.newPage();
pa.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
pa.on('console', (m) => m.type() === 'error' && !/400|401|403|404|Failed to load resource/.test(m.text()) && errors.push(m.text()));
pages.push(pa);
await pa.goto(B + '/login'); await pa.fill('#email', 'admin@acme.test'); await pa.fill('#password', 'supersecret1');
await pa.click('button:has-text("Sign in")'); await pa.waitForURL(B + '/');

try {
  await step('a new robot says how often it would have run', async () => {
    await pa.goto(B + '/crm/settings');
    await pa.click('a[href="?tab=automation"]'); await pa.locator('button[role=tab]:has-text("Rules")').waitFor();
    await pa.selectOption('select[aria-label="Pipeline"]', pipe.id);
    await pa.locator(`section[aria-label="Stage ${first.name}"] >> button:has-text("+ Robot")`).click();
    await pa.click('[data-preview-run]');
    await pa.locator('[data-preview-result]:has-text("Would have run 3 times, on 3 records")').waitFor();
    await pa.keyboard.press('Escape');
  });
  await step('no browser errors', async () => { if (errors.length) throw new Error(errors.join(' | ')); });
} finally {
  await browser.close();
  await db.query("DELETE FROM crm_items WHERE title LIKE $1", [`Pv % ${s}`]).catch(() => {});
  await db.query("DELETE FROM crm_categories WHERE id = $1", [pipe.id]).catch(() => {});
  await db.end();
  await cleanupRun(s);
}
console.log(failed ? `\n${failed} failed` : '\nall passed');
process.exit(failed ? 1 : 0);
