// Promise checker in the browser: from a deal, check its close date, see why it can't be done
// and make the earliest workable date the close date.
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
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `${OUTPUT}/fail83-${n}-${i}.png`, fullPage: true }).catch(() => {}); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
const due = new Date(Date.now() + 14 * 86400e3).toISOString().slice(0, 10);
const u = (await admin.post('/users', { email: `ivo.${s}@acme.test`, password: 'password123', first_name: 'Ivo', last_name: `P${s}` })).data;
const work = (await admin.post('/crm/products', { name: `Install ${s}`, price: '50', unit: 'h' })).data;
await db.query('UPDATE crm_products SET is_service = TRUE WHERE id = $1', [work.id]);
const deal = (await admin.post('/crm/deal/items', { title: `Big install ${s}`, assigned_to: u.id, close_date: due })).data;
await admin.put(`/crm/items/${deal.id}/products`, { rows: [{ product_id: work.id, name: `Install ${s}`, price: '50', quantity: '200', unit: 'h' }] });

const ctx = await browser.newContext({ viewport: { width: 1400, height: 950 }, timezoneId: 'UTC', locale: 'en-GB' });
const pa = await ctx.newPage();
pa.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
pa.on('console', (m) => m.type() === 'error' && !/400|401|403|404|Failed to load resource/.test(m.text()) && errors.push(m.text()));
pages.push(pa);
await pa.goto(B + '/login'); await pa.fill('#email', 'admin@acme.test'); await pa.fill('#password', 'supersecret1');
await pa.click('button:has-text("Sign in")'); await pa.waitForURL(B + '/');

try {
  let earliest = '';
  await step('checking the close date', async () => {
    await pa.goto(`${B}/crm/deal/${deal.id}`);
    const box = pa.locator('section[aria-label="Promise checker"]');
    await box.locator('[data-promise-open]').click();
    await box.locator('[data-promise="no"]').waitFor();
    await box.locator(`[data-person="Ivo P${s}"]`).waitFor();
    await box.locator('text=200 hours of work').waitFor();
    earliest = (await box.locator('[data-earliest]').getAttribute('data-earliest')) ?? '';
    if (!(earliest > due)) throw new Error(`earliest ${earliest}`);
  });

  await step('making the earliest date the close date', async () => {
    const box = pa.locator('section[aria-label="Promise checker"]');
    await box.locator('[data-promise-earliest]').click();
    await box.locator('[data-promise="ok"]').waitFor();
    const d = (await admin.get(`/crm/deal/items/${deal.id}`)).data;
    if (d.close_date !== earliest) throw new Error(`close date ${d.close_date}, earliest ${earliest}`);
  });

  await step('no browser errors', async () => { if (errors.length) throw new Error(errors.join(' | ')); });
} finally {
  await browser.close();
  await db.query("DELETE FROM crm_items WHERE title = $1", [`Big install ${s}`]).catch(() => {});
  await db.query('DELETE FROM crm_products WHERE id = $1', [work.id]).catch(() => {});
  await db.end();
  await cleanupRun(s);
}
console.log(failed ? `\n${failed} failed` : '\nall passed');
process.exit(failed ? 1 : 0);
