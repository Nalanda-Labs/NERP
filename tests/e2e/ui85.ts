// Personal data requests in the browser: the public form, and staff erasing a person's data
// after reviewing what was found.
import { chromium } from 'playwright';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { OUTPUT } from './output.ts';
import { dbUrl } from '../db/cleanup.ts';
import pg from 'pg';
import { execFileSync } from 'node:child_process';
const B = 'http://localhost:8080';
const s = Date.now().toString(36);
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: string[] = [];
let failed = 0, n = 0;
const pages: import('playwright').Page[] = [];
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `${OUTPUT}/fail85-${n}-${i}.png`, fullPage: true }).catch(() => {}); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
// The public form's per-address limits count every local run: start from a clean slate.
try {
  const url = process.env.REDIS_URL ?? 'redis://127.0.0.1:6379';
  const keys = execFileSync('redis-cli', ['-u', url, '--scan', '--pattern', 'privacy:*']).toString().split('\n').filter(Boolean);
  if (keys.length) execFileSync('redis-cli', ['-u', url, 'DEL', ...keys]);
} catch { /* no redis-cli: the limits pass within the hour */ }
const email = `ezra.${s}@example.com`, pub = `pia.${s}@example.com`;
const ezra = (await admin.post('/crm/contact/items', { first_name: 'Ezra', last_name: `U${s}`, multifields: [{ kind: 'email', value_type: 'work', value: email }] })).data;

const ctx = await browser.newContext({ viewport: { width: 1400, height: 950 }, timezoneId: 'UTC', locale: 'en-GB' });
const pa = await ctx.newPage();
pa.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
pa.on('console', (m) => m.type() === 'error' && !/400|401|403|404|Failed to load resource/.test(m.text()) && errors.push(m.text()));
pages.push(pa);

try {
  await step('the public form', async () => {
    await pa.goto(`${B}/privacy-request`);
    await pa.check('[data-erasure]');
    await pa.fill('input[name="email"]', pub);
    await pa.fill('input[name="name"]', 'Pia');
    await pa.click('button:has-text("Send the request")');
    await pa.locator('[data-sent]').waitFor();
    const job = (await db.query("SELECT payload FROM jobs WHERE kind = 'email.send' AND payload->>'to' = $1", [pub])).rows[0]?.payload;
    const link = /(\/privacy-request\/confirm\?token=[A-Za-z0-9_-]+)/.exec(job?.text ?? '')?.[1];
    if (!link) throw new Error('no confirmation email');
    await pa.goto(B + link);
    await pa.locator('[data-confirmed]').waitFor();
  });

  await pa.goto(B + '/login'); await pa.fill('#email', 'admin@acme.test'); await pa.fill('#password', 'supersecret1');
  await pa.click('button:has-text("Sign in")'); await pa.waitForURL(B + '/');

  await step('the confirmed request is listed', async () => {
    await pa.click('a[href="/privacy"]');
    await pa.waitForURL(/\/privacy$/);
    await pa.locator(`[data-request="${pub}"]`).waitFor();
  });

  await step('staff erase someone\'s data after reviewing it', async () => {
    await pa.click('[data-new-request]');
    await pa.selectOption('select[name="kind"]', 'erasure');
    await pa.fill('form[aria-label="New request"] input[name="email"]', email);
    await pa.click('button:has-text("Add and look them up")');
    await pa.waitForURL(/\/privacy\/[0-9a-f-]{36}$/);
    await pa.locator('[data-source="crm"]').waitFor();
    if (!(await pa.locator('[data-erase]').isDisabled())) throw new Error('erase should wait for the confirmation');
    await pa.fill('[data-confirm]', email);
    await pa.click('[data-erase]');
    await pa.locator('[data-status="done"]').waitFor();
    await pa.locator('[data-outcome]').waitFor();
    const c = (await db.query('SELECT first_name FROM crm_items WHERE id = $1', [ezra.id])).rows[0];
    if (c.first_name !== 'Erased') throw new Error(`still ${c.first_name}`);
  });

  await step('no browser errors', async () => { if (errors.length) throw new Error(errors.join(' | ')); });
} finally {
  await browser.close();
  await db.query('DELETE FROM privacy_requests WHERE email IN ($1, $2)', [email, pub]).catch(() => {});
  await db.query('DELETE FROM mkt_optouts WHERE lower(email) = $1', [email]).catch(() => {});
  await db.query("DELETE FROM jobs WHERE kind = 'email.send' AND payload->>'to' = $1", [pub]).catch(() => {});
  await db.query('DELETE FROM crm_items WHERE id = $1', [ezra.id]).catch(() => {});
  await db.end();
  await cleanupRun(s);
}
console.log(failed ? `\n${failed} failed` : '\nall passed');
process.exit(failed ? 1 : 0);
