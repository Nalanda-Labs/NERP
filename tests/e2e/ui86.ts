// Retention rules in the browser: reached from Data requests, turning a rule on with its days,
// applying now and seeing it in the log.
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
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `${OUTPUT}/fail86-${n}-${i}.png`, fullPage: true }).catch(() => {}); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
const before = (await db.query('SELECT * FROM retention_rules')).rows;
// One form submission old enough for the rule below, and nothing else that old.
const form = (await db.query("INSERT INTO crm_webforms (key, name, entity_type) VALUES ($1, $2, 3) RETURNING id", [`uf${s}`, `UI form ${s}`])).rows[0].id;
await db.query("INSERT INTO crm_webform_submissions (form_id, data, created_at) VALUES ($1, '{}', now() - interval '9 years')", [form]);

const ctx = await browser.newContext({ viewport: { width: 1400, height: 950 }, timezoneId: 'UTC', locale: 'en-GB' });
const pa = await ctx.newPage();
pa.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
pa.on('console', (m) => m.type() === 'error' && !/400|401|403|404|Failed to load resource/.test(m.text()) && errors.push(m.text()));
pages.push(pa);
await pa.goto(B + '/login'); await pa.fill('#email', 'admin@acme.test'); await pa.fill('#password', 'supersecret1');
await pa.click('button:has-text("Sign in")'); await pa.waitForURL(B + '/');

try {
  await step('turning a rule on', async () => {
    await pa.goto(`${B}/privacy`);
    await pa.click('a[href="/privacy/retention"]');
    const row = pa.locator('[data-rule="form_submissions"]');
    await row.waitFor();
    await row.locator('[data-days]').fill('3000');
    await row.locator('[data-days]').press('Tab');
    await row.locator('[data-due]:has-text("older than this now")').waitFor();
    await row.locator('[data-enabled]').check();
    for (let i = 0; i < 20; i++) {
      const r = (await db.query("SELECT enabled, keep_days FROM retention_rules WHERE key = 'form_submissions'")).rows[0];
      if (r?.enabled && r.keep_days === 3000) return;
      await new Promise((res) => setTimeout(res, 250));
    }
    throw new Error('not saved');
  });

  await step('applying now', async () => {
    await pa.click('[data-run-retention]');
    await pa.locator('text=/\\d+ records handled/').waitFor();
    await pa.locator('[data-rule="form_submissions"] [data-due]:has-text("Nothing older")').waitFor();
    if ((await db.query('SELECT 1 FROM crm_webform_submissions WHERE form_id = $1', [form])).rowCount !== 0) throw new Error('still there');
  });

  await step('no browser errors', async () => { if (errors.length) throw new Error(errors.join(' | ')); });
} finally {
  await browser.close();
  await db.query('DELETE FROM retention_rules').catch(() => {});
  for (const x of before) await db.query('INSERT INTO retention_rules (key, enabled, keep_days, action, updated_by, updated_at) VALUES ($1, $2, $3, $4, $5, $6)', [x.key, x.enabled, x.keep_days, x.action, x.updated_by, x.updated_at]).catch(() => {});
  await db.query('DELETE FROM crm_webforms WHERE id = $1', [form]).catch(() => {});
  await db.end();
  await cleanupRun(s);
}
console.log(failed ? `\n${failed} failed` : '\nall passed');
process.exit(failed ? 1 : 0);
