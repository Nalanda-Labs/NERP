// Client portal in the browser: staff email a contact the sign-in link from the contact page;
// the client opens it and sees their invoice; signed out, the portal page offers a new link.
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
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `${OUTPUT}/fail90-${n}-${i}.png`, fullPage: true }).catch(() => {}); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
const base = (await admin.get('/crm/config')).data.base_currency;
const email = `oli.${s}@example.com`;
const oli = (await admin.post('/crm/contact/items', { first_name: 'Oli', last_name: `Q${s}`, multifields: [{ kind: 'email', value_type: 'work', value: email }] })).data;
const inv = (await admin.post('/crm/invoice/items', { title: `Oli's invoice ${s}`, contact_ids: [oli.id], currency: base, opportunity: '120', is_manual_amount: true })).data;

const ctx = await browser.newContext({ viewport: { width: 1400, height: 950 }, timezoneId: 'UTC', locale: 'en-GB' });
const pa = await ctx.newPage();
const client = await (await browser.newContext({ viewport: { width: 900, height: 900 }, locale: 'en-GB' })).newPage();
for (const p of [pa, client]) {
  p.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
  p.on('console', (m) => m.type() === 'error' && !/400|401|403|404|Failed to load resource/.test(m.text()) && errors.push(m.text()));
}
pages.push(pa, client);
await pa.goto(B + '/login'); await pa.fill('#email', 'admin@acme.test'); await pa.fill('#password', 'supersecret1');
await pa.click('button:has-text("Sign in")'); await pa.waitForURL(B + '/');

try {
  let link = '';
  await step('staff send the sign-in link from the contact', async () => {
    await pa.goto(`${B}/crm/contact/${oli.id}`);
    const card = pa.locator('section[aria-label="Client portal"]');
    await card.locator('[data-portal-invite]').click();
    const input = card.locator('[aria-label="Sign-in link"]');
    await input.waitFor();
    link = await input.inputValue();
    if (!link.includes('/portal/sign-in?token=')) throw new Error(link);
    if (!(await card.locator('[data-portal-enabled]').isChecked())) throw new Error('not turned on');
  });

  await step('the client opens it and sees their invoice', async () => {
    await client.goto(link.replace(/^https?:\/\/[^/]+/, B));
    await client.waitForURL(/\/portal$/);
    await client.locator('[data-portal-name]').filter({ hasText: `Oli Q${s}` }).waitFor();
    await client.locator(`section[aria-label="Invoices"] >> text=Oli's invoice ${s}`).waitFor();
  });

  await step('the link doesn\'t work twice; signed out, a new one can be asked for', async () => {
    await client.click('button:has-text("Sign out")');
    await client.locator('[aria-label="Email"]').waitFor();
    await client.goto(link.replace(/^https?:\/\/[^/]+/, B));
    await client.locator('[data-link-bad]').waitFor();
    await client.goto(`${B}/portal`);
    await client.fill('[aria-label="Email"]', email);
    await client.click('button:has-text("Send me a link")');
    await client.locator('[data-link-sent]').waitFor();
  });

  await step('no browser errors', async () => { if (errors.length) throw new Error(errors.join(' | ')); });
} finally {
  await browser.close();
  await db.query("DELETE FROM jobs WHERE kind = 'email.send' AND payload->>'to' = $1", [email]).catch(() => {});
  await db.query('DELETE FROM crm_items WHERE id = ANY($1)', [[oli.id, inv.id]]).catch(() => {});
  await db.end();
  await cleanupRun(s);
}
console.log(failed ? `\n${failed} failed` : '\nall passed');
process.exit(failed ? 1 : 0);
