// Capacity-aware assignment in the browser: skills on an employee's profile, and the assign
// robot's "who's in" mode with its preview.
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
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `${OUTPUT}/fail88-${n}-${i}.png`, fullPage: true }).catch(() => {}); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
const tz = (await db.query("SELECT value #>> '{}' AS v FROM settings WHERE key = 'default_timezone'")).rows[0]?.v ?? 'UTC';
const today = new Intl.DateTimeFormat('en-CA', { timeZone: tz }).format(new Date());
const kai = (await admin.post('/users', { email: `kai.${s}@acme.test`, password: 'password123', first_name: 'Kai', last_name: `G${s}` })).data;
const shift = (await db.query("INSERT INTO hr_shifts (name, start_min, end_min, break_minutes, color, is_active) VALUES ($1, 540, 1020, 60, '#888', TRUE) RETURNING id", [`UI day ${s}`])).rows[0].id;
await db.query('INSERT INTO hr_rota (user_id, day, shift_id) VALUES ($1, $2, $3)', [kai.id, today, shift]);

const ctx = await browser.newContext({ viewport: { width: 1400, height: 950 }, timezoneId: 'UTC', locale: 'en-GB' });
const pa = await ctx.newPage();
pa.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
pa.on('console', (m) => m.type() === 'error' && !/400|401|403|404|Failed to load resource/.test(m.text()) && errors.push(m.text()));
pages.push(pa);
await pa.goto(B + '/login'); await pa.fill('#email', 'admin@acme.test'); await pa.fill('#password', 'supersecret1');
await pa.click('button:has-text("Sign in")'); await pa.waitForURL(B + '/');

try {
  await step('skills on the profile', async () => {
    await pa.goto(`${B}/employees/${kai.id}`);
    const box = pa.locator('section[aria-label="Skills"]');
    await box.locator('[aria-label="New skill"]').fill('Spanish');
    await box.locator('button:has-text("Add")').click();
    await box.locator('[data-skill="Spanish"]').waitFor();
    if ((await admin.get(`/users/${kai.id}/skills`)).data.skills[0] !== 'Spanish') throw new Error('not saved');
  });

  await step('the assign robot\'s "who\'s in" mode and its preview', async () => {
    await pa.goto(`${B}/crm/settings`);
    await pa.click('a[href="?tab=automation"]');
    await pa.locator('button[role=tab]:has-text("Rules")').waitFor();
    await pa.locator('section[aria-label^="Stage "] >> button:has-text("+ Robot")').first().click();
    await pa.selectOption('#au-action', 'assign');
    await pa.fill('dialog input[placeholder="+ Employee…"]', `Kai G${s}`);
    await pa.click(`dialog ul >> button:has-text("Kai G${s}")`);
    await pa.check('[data-mode-capacity]');
    await pa.locator('[data-capacity]').waitFor();
    await pa.click('[data-assign-preview]');
    await pa.locator(`[data-preview-person="Kai G${s}"]:has-text("working today")`).waitFor();
    await pa.locator(`[data-preview-person="Kai G${s}"]:has-text("Spanish")`).waitFor();
    await pa.locator('[data-preview-how]:has-text("fewest open")').waitFor();
    await pa.keyboard.press('Escape');
  });

  await step('no browser errors', async () => { if (errors.length) throw new Error(errors.join(' | ')); });
} finally {
  await browser.close();
  await db.query('DELETE FROM hr_rota WHERE shift_id = $1', [shift]).catch(() => {});
  await db.query('DELETE FROM hr_shifts WHERE id = $1', [shift]).catch(() => {});
  await db.end();
  await cleanupRun(s);
}
console.log(failed ? `\n${failed} failed` : '\nall passed');
process.exit(failed ? 1 : 0);
