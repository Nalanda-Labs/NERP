// Teams in the browser: from the company structure to Teams, make a team with a lead, add
// someone from another department with a role, open the team chat; the profile lists it.
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
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `${OUTPUT}/fail94-${n}-${i}.png`, fullPage: true }).catch(() => {}); }
};
const admin = new Client();
const me = (await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' })).data.user;
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
const ida = (await admin.post('/users', { email: `ida.${s}@acme.test`, password: 'password123', first_name: 'Ida', last_name: `Tm${s}` })).data;

const ctx = await browser.newContext({ viewport: { width: 1400, height: 950 }, timezoneId: 'UTC', locale: 'en-GB' });
const pa = await ctx.newPage();
pa.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
pa.on('console', (m) => m.type() === 'error' && !/400|401|403|404|Failed to load resource/.test(m.text()) && errors.push(m.text()));
pages.push(pa);
await pa.goto(B + '/login'); await pa.fill('#email', 'admin@acme.test'); await pa.fill('#password', 'supersecret1');
await pa.click('button:has-text("Sign in")'); await pa.waitForURL(B + '/');

try {
  await step('a team made from the structure page', async () => {
    await pa.goto(`${B}/structure`);
    await pa.click('[data-teams-link]');
    await pa.waitForURL(/\/teams$/);
    await pa.click('[data-new-team]');
    await pa.fill('form[aria-label="New team"] input[name="name"]', `Squad ${s}`);
    await pa.fill('form[aria-label="New team"] input[placeholder="Choose the lead…"]', me.first_name);
    await pa.click(`form[aria-label="New team"] ul >> button:has-text("${me.first_name}")`);
    await pa.click('button:has-text("Create the team")');
    await pa.locator(`section[aria-label="Team"] >> text=Squad ${s}`).waitFor();
  });

  await step('someone from another department, with a role', async () => {
    const box = pa.locator('section[aria-label="Team"]');
    await box.locator('input[placeholder^="+ Add someone"]').fill(`Ida Tm${s}`);
    await box.locator(`ul >> button:has-text("Ida Tm${s}")`).click();
    await box.locator(`[data-member="Ida Tm${s}"]`).waitFor();
    await box.locator(`[aria-label="Role of Ida Tm${s}"]`).fill('Research');
    await box.locator(`[aria-label="Role of Ida Tm${s}"]`).press('Tab');
    await box.locator(`[data-member="Ida Tm${s}"]:has-text("Research")`).waitFor();
  });

  await step('the team chat', async () => {
    await pa.click('[data-team-chat]');
    await pa.waitForURL(/\/messenger\?chat=/);
  });

  await step('the profile lists the team', async () => {
    await pa.goto(`${B}/employees/${ida.id}`);
    await pa.locator(`[data-profile-team="Squad ${s}"]`).waitFor();
  });

  await step('no browser errors', async () => { if (errors.length) throw new Error(errors.join(' | ')); });
} finally {
  await browser.close();
  await db.query('DELETE FROM teams WHERE name = $1', [`Squad ${s}`]).catch(() => {});
  await db.end();
  await cleanupRun(s);
}
console.log(failed ? `\n${failed} failed` : '\nall passed');
process.exit(failed ? 1 : 0);
