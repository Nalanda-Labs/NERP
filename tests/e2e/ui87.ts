// Duplicate prevention in the browser: typing a new contact shows a lead with the same phone,
// and someone who emailed but isn't in the CRM; creating it attaches their emails.
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
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `${OUTPUT}/fail87-${n}-${i}.png`, fullPage: true }).catch(() => {}); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
const last = `Ashgrove${s}`;
const who = `wren.${s}@example.com`;
const digits = String(Date.now()).slice(-7);
const lead = (await admin.post('/crm/lead/items', { title: `Lead ${s}`, first_name: 'Wren', last_name: last, multifields: [{ kind: 'phone', value_type: 'work', value: `+1 555 ${digits}` }] })).data;
const mail = (await db.query("INSERT INTO crm_emails (direction, status, message_id, thread_id, subject, from_addr, sent_at) VALUES ('in', 'received', $1, gen_random_uuid(), $2, $3, now() - interval '1 day') RETURNING id", [`<w-${s}@x>`, `Garden design ${s}`, who])).rows[0].id;

const ctx = await browser.newContext({ viewport: { width: 1400, height: 950 }, timezoneId: 'UTC', locale: 'en-GB' });
const pa = await ctx.newPage();
pa.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
pa.on('console', (m) => m.type() === 'error' && !/400|401|403|404|Failed to load resource/.test(m.text()) && errors.push(m.text()));
pages.push(pa);
await pa.goto(B + '/login'); await pa.fill('#email', 'admin@acme.test'); await pa.fill('#password', 'supersecret1');
await pa.click('button:has-text("Sign in")'); await pa.waitForURL(B + '/');

try {
  await step('a lead with the same phone, while typing a contact', async () => {
    await pa.goto(`${B}/crm/contact/new`);
    await pa.fill('#crm-first', 'Wren'); await pa.fill('#crm-last', last);
    await pa.click('text=+ Add phone'); await pa.fill('input[type=tel]', `555${digits}`);
    const box = pa.locator('[data-dups]');
    await box.locator(`[data-dup="Lead ${s}"]`).waitFor();
    await box.locator('text=Lead').first().waitFor();
    await box.locator('text=same phone').waitFor();
  });

  await step('someone who emailed but isn\'t in the CRM, attached on creating', async () => {
    await pa.click('text=+ Add email'); await pa.fill('input[type=email]', who);
    await pa.locator('[data-history]').waitFor();
    await pa.locator(`[data-history]:has-text("Garden design ${s}")`).waitFor();
    await pa.click('button:has-text("Create contact")');
    await pa.waitForURL(/\/crm\/contact\/[0-9a-f-]{36}$/);
    const id = pa.url().split('/').pop();
    for (let i = 0; i < 20; i++) {
      if ((await db.query('SELECT 1 FROM crm_email_links WHERE email_id = $1 AND item_id = $2', [mail, id])).rowCount === 1) return;
      await new Promise((r) => setTimeout(r, 250));
    }
    throw new Error('the email wasn\'t attached');
  });

  await step('no browser errors', async () => { if (errors.length) throw new Error(errors.join(' | ')); });
} finally {
  await browser.close();
  await db.query('DELETE FROM crm_emails WHERE id = $1', [mail]).catch(() => {});
  await db.query('DELETE FROM crm_items WHERE last_name = $1 OR id = $2', [last, lead.id]).catch(() => {});
  await db.end();
  await cleanupRun(s);
}
console.log(failed ? `\n${failed} failed` : '\nall passed');
process.exit(failed ? 1 : 0);
