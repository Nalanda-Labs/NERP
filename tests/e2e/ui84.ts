// One client, everything, in the browser: a company's history card with a deal's comment and
// a contact's call, filtered by kind.
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
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `${OUTPUT}/fail84-${n}-${i}.png`, fullPage: true }).catch(() => {}); }
};
const admin = new Client();
const me = (await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' })).data.user;
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
const co = (await admin.post('/crm/company/items', { company_name: `Whole Co ${s}` })).data;
const ann = (await admin.post('/crm/contact/items', { first_name: 'Ann', last_name: `W${s}`, company_id: co.id })).data;
const deal = (await admin.post('/crm/deal/items', { title: `Whole deal ${s}`, company_id: co.id })).data;
await db.query("INSERT INTO crm_timeline (item_id, kind, author_id, body) VALUES ($1, 'comment', $2, $3)", [deal.id, me.id, `Deal note ${s}`]);
await db.query("INSERT INTO crm_activities (item_id, kind, subject, responsible_id, created_by, is_completed, completed_at) VALUES ($1, 'call', $2, $3, $3, TRUE, now() - interval '1 hour')", [ann.id, `Called Ann ${s}`, me.id]);

const ctx = await browser.newContext({ viewport: { width: 1400, height: 950 }, timezoneId: 'UTC', locale: 'en-GB' });
const pa = await ctx.newPage();
pa.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
pa.on('console', (m) => m.type() === 'error' && !/400|401|403|404|Failed to load resource/.test(m.text()) && errors.push(m.text()));
pages.push(pa);
await pa.goto(B + '/login'); await pa.fill('#email', 'admin@acme.test'); await pa.fill('#password', 'supersecret1');
await pa.click('button:has-text("Sign in")'); await pa.waitForURL(B + '/');

try {
  await step('the company\'s whole history', async () => {
    await pa.goto(`${B}/crm/company/${co.id}`);
    const box = pa.locator('section[aria-label="Everything"]');
    await box.locator(`text=Deal note ${s}`).waitFor();
    await box.locator(`text=Called Ann ${s}`).waitFor();
  });

  await step('filtered by kind', async () => {
    const box = pa.locator('section[aria-label="Everything"]');
    await box.locator('[data-kind="activity"]').click();
    await pa.waitForFunction((t) => !document.querySelector('section[aria-label="Everything"]')?.textContent?.includes(t), `Deal note ${s}`, { timeout: 10000 });
    await box.locator(`text=Called Ann ${s}`).waitFor();
    // Back to all, and through to the deal.
    await box.locator('[data-kind="activity"]').click();
    await box.locator(`a:has-text("Comment by")`).first().click();
    await pa.waitForURL(new RegExp(`/crm/deal/${deal.id}`));
  });

  await step('no browser errors', async () => { if (errors.length) throw new Error(errors.join(' | ')); });
} finally {
  await browser.close();
  await db.query("DELETE FROM crm_items WHERE title = $1 OR last_name = $2 OR company_name = $3", [`Whole deal ${s}`, `W${s}`, `Whole Co ${s}`]).catch(() => {});
  await db.end();
  await cleanupRun(s);
}
console.log(failed ? `\n${failed} failed` : '\nall passed');
process.exit(failed ? 1 : 0);
