// Field visits in the browser: add a visit to a client, plan the day, check in where the
// browser says we are, and check out with notes.
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
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `${OUTPUT}/fail91-${n}-${i}.png`, fullPage: true }).catch(() => {}); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
await admin.post('/users', { email: `fin.${s}@acme.test`, password: 'password123', first_name: 'Fin', last_name: `V${s}` });
const fin = new Client();
await fin.post('/auth/login', { email: `fin.${s}@acme.test`, password: 'password123' });
const today = new Date().toISOString().slice(0, 10);
const north = (await admin.post('/crm/company/items', { company_name: `North ${s}` })).data;
const south = (await admin.post('/crm/company/items', { company_name: `South ${s}` })).data;
await fin.post('/field/visits', { item_id: north.id, day: today, lat: 51.52, lng: -0.10 });

const ctx = await browser.newContext({ viewport: { width: 1400, height: 950 }, timezoneId: 'UTC', locale: 'en-GB', geolocation: { latitude: 51.52, longitude: -0.10 }, permissions: ['geolocation'] });
const pa = await ctx.newPage();
pa.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
pa.on('console', (m) => m.type() === 'error' && !/400|401|403|404|Failed to load resource/.test(m.text()) && errors.push(m.text()));
pages.push(pa);
await pa.goto(B + '/login'); await pa.fill('#email', `fin.${s}@acme.test`); await pa.fill('#password', 'password123');
await pa.click('button:has-text("Sign in")'); await pa.waitForURL(B + '/');

try {
  await step('adding a visit to a client', async () => {
    await pa.goto(`${B}/crm/visits`);
    await pa.locator(`[data-visit="North ${s}"]`).waitFor();
    await pa.fill('section[aria-label="Add a visit"] input', `South ${s}`);
    await pa.click(`section[aria-label="Add a visit"] >> button:has-text("South ${s}")`);
    await pa.click('[data-add-visit]');
    await pa.locator(`[data-visit="South ${s}"]`).waitFor();
  });

  await step('planning the day', async () => {
    await pa.click('[data-plan-day]');
    await pa.locator('text=The day is planned').waitFor();
    await pa.locator('[data-totals]:has-text("2 visits")').waitFor();
  });

  await step('checking in and out', async () => {
    const row = pa.locator(`[data-visit="North ${s}"]`);
    await row.locator('[data-check-in]').click();
    await row.locator('text=0 m from the address').waitFor();
    await row.locator('[aria-label="Notes"]').fill(`Signed the order ${s}`);
    await row.locator('[data-check-out]').click();
    await row.locator(`text=Signed the order ${s}`).waitFor();
    const t = await db.query("SELECT 1 FROM crm_timeline WHERE item_id = $1 AND body LIKE '%' || $2 || '%'", [north.id, `Signed the order ${s}`]);
    if (t.rowCount !== 1) throw new Error('not logged on the record');
  });

  await step('no browser errors', async () => { if (errors.length) throw new Error(errors.join(' | ')); });
} finally {
  await browser.close();
  await db.query('DELETE FROM crm_items WHERE id = ANY($1)', [[north.id, south.id]]).catch(() => {});
  await db.end();
  await cleanupRun(s);
}
console.log(failed ? `\n${failed} failed` : '\nall passed');
process.exit(failed ? 1 : 0);
