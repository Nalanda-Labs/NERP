// Commissions and credit notes in the browser: a plan made on the Commissions page earns on a
// paid invoice; a credit note posted from the invoice takes it back; the month is locked.
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
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `${OUTPUT}/fail89-${n}-${i}.png`, fullPage: true }).catch(() => {}); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
const base = (await admin.get('/crm/config')).data.base_currency;
const today = new Date().toISOString().slice(0, 10);
const month = today.slice(0, 7);
const made: { items: string[]; payments: string[] } = { items: [], payments: [] };
const ready = (await admin.get('/inventory/settings')).data.accounting_ready;
const sam = (await admin.post('/users', { email: `sam.${s}@acme.test`, password: 'password123', first_name: 'Sam', last_name: `Ui${s}` })).data;
const wasLocked = (await db.query("SELECT 1 FROM comm_months WHERE to_char(month, 'YYYY-MM') = $1", [month])).rowCount;

const ctx = await browser.newContext({ viewport: { width: 1400, height: 950 }, timezoneId: 'UTC', locale: 'en-GB', acceptDownloads: true });
const pa = await ctx.newPage();
pa.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
pa.on('console', (m) => m.type() === 'error' && !/400|401|403|404|Failed to load resource/.test(m.text()) && errors.push(m.text()));
pages.push(pa);
await pa.goto(B + '/login'); await pa.fill('#email', 'admin@acme.test'); await pa.fill('#password', 'supersecret1');
await pa.click('button:has-text("Sign in")'); await pa.waitForURL(B + '/');

try {
  if (!ready) console.log('skipped: accounting is not set up');
  else {
    await step('a plan made on the Commissions page', async () => {
      await pa.click('a[href="/crm/commissions"]');
      await pa.click('[data-tab="plans"]');
      await pa.click('[data-new-plan]');
      await pa.fill('form[aria-label="Plan"] input[name="name"]', `Ui plan ${s}`);
      await pa.fill('form[aria-label="Plan"] input[name="rate"]', '10');
      await pa.fill('form[aria-label="Plan"] input[placeholder="Add people…"]', `Sam Ui${s}`);
      await pa.click(`form[aria-label="Plan"] ul >> button:has-text("Sam Ui${s}")`);
      await pa.click('button:has-text("Save plan")');
      await pa.locator(`[data-plan="Ui plan ${s}"]`).waitFor();
    });

    // An invoice of Sam's for 300, posted and paid.
    const bank = (await admin.get('/accounting/accounts')).data.items.find((a: any) => a.subtype === 'bank' && a.is_active);
    const client = (await admin.post('/crm/company/items', { company_name: `Ui buyer ${s}` })).data;
    const inv = (await admin.post('/crm/invoice/items', { title: `Ui invoice ${s}`, company_id: client.id, currency: base, assigned_to: sam.id, opportunity: '300', is_manual_amount: true })).data;
    made.items.push(inv.id);
    await admin.post(`/accounting/invoices/${inv.id}/post`, { date: today });
    const pay = await admin.post('/accounting/payments', { direction: 'in', date: today, account_id: bank.id, amount: '300.00', allocations: [{ doc_kind: 'invoice', doc_id: inv.id, amount: '300.00' }] });
    made.payments.push(pay.data.id);

    await step('the paid invoice earns, on the person\'s statement', async () => {
      await pa.goto(`${B}/crm/commissions`);
      await pa.click('[data-tab="team"]');
      await pa.click(`[data-person="Sam Ui${s}"] button`);
      await pa.locator(`[data-net]:has-text("30.00")`).waitFor();
    });

    await step('a credit note from the invoice takes it back', async () => {
      await pa.goto(`${B}/crm/invoice/${inv.id}`);
      await pa.click('[data-credit-open]');
      await pa.fill('[aria-label="Amount to credit"]', '100');
      await pa.fill('form[aria-label="Credit note"] [aria-label="Reason"]', 'Returned');
      await pa.click('button:has-text("Post the credit note")');
      await pa.locator('[data-books-credited]').waitFor();
      await pa.locator('[data-credit-notes]:has-text("Returned")').waitFor();
      await pa.goto(`${B}/crm/commissions`);
      await pa.click('[data-tab="team"]');
      await pa.click(`[data-person="Sam Ui${s}"] button`);
      await pa.locator(`[data-net]:has-text("20.00")`).waitFor();
      await pa.locator(`[data-taken-back]:has-text("10.00")`).waitFor();
    });

    if (!wasLocked) {
      await step('locking the month for payroll', async () => {
        await pa.click('[data-tab="team"]');
        await pa.click('[data-lock]');
        await pa.locator('[data-locked]').waitFor();
        await pa.click('[data-lock]');
        await pa.waitForFunction(() => !document.querySelector('[data-locked]'));
      });
    }
  }
  await step('no browser errors', async () => { if (errors.length) throw new Error(errors.join(' | ')); });
} finally {
  await browser.close();
  const plans = (await db.query('SELECT id FROM comm_plans WHERE name = $1', [`Ui plan ${s}`])).rows.map((r) => r.id);
  await db.query('DELETE FROM comm_entries WHERE plan_id = ANY($1)', [plans]).catch(() => {});
  await db.query('DELETE FROM comm_plans WHERE id = ANY($1)', [plans]).catch(() => {});
  if (!wasLocked) await db.query("DELETE FROM comm_months WHERE to_char(month, 'YYYY-MM') = $1", [month]).catch(() => {});
  const entries = (await db.query('SELECT id FROM acc_entries WHERE source_id = ANY($1) OR source_id = ANY($2)', [made.items, made.payments])).rows.map((r) => r.id);
  await db.query('DELETE FROM acc_credit_notes WHERE invoice_id = ANY($1)', [made.items]).catch(() => {});
  await db.query('DELETE FROM acc_allocations WHERE payment_id = ANY($1)', [made.payments]).catch(() => {});
  await db.query('DELETE FROM acc_payments WHERE id = ANY($1)', [made.payments]).catch(() => {});
  await db.query('DELETE FROM acc_invoices WHERE item_id = ANY($1)', [made.items]).catch(() => {});
  if (entries.length) {
    await db.query('DELETE FROM acc_lines WHERE entry_id = ANY($1)', [entries]).catch(() => {});
    await db.query('UPDATE acc_entries SET reverses = NULL, reversed_by = NULL WHERE id = ANY($1)', [entries]).catch(() => {});
    await db.query('DELETE FROM acc_entries WHERE id = ANY($1)', [entries]).catch(() => {});
  }
  await db.query('DELETE FROM crm_items WHERE id = ANY($1) OR company_name = $2', [made.items, `Ui buyer ${s}`]).catch(() => {});
  await db.end();
  await cleanupRun(s);
}
console.log(failed ? `\n${failed} failed` : '\nall passed');
process.exit(failed ? 1 : 0);
