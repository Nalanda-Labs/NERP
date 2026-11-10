// Accounting in the browser: a CRM invoice posted and paid (and shown so on the invoice), a
// supplier's bill entered and posted, a journal entry written, posted and reversed, a bank
// statement imported and a line recorded, and the reports (balance sheet balances).
import { chromium } from 'playwright';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
const B = 'http://localhost:8080';
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const s = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const st = (await admin.get('/accounting/settings')).data;
if (!st.settings.ready) await admin.post('/accounting/setup', { fiscal_year_start: 1 });
const base = st.base_currency;
const acc = async (code: string, name: string, subtype: string) => (await admin.post('/accounting/accounts', { code: `U${s}-${code}`, name: `${name} ${s}`, subtype })).data.id;
const bank = await acc('1010', 'UI bank', 'bank');
const costs = await acc('6000', 'UI costs', 'expense');
const taxAcc = await acc('2100', 'UI tax', 'tax_payable');
await admin.post('/accounting/taxes', { name: `UI tax ${s}`, rate: '10', scope: 'sales', account_id: taxAcc });
const company = (await admin.post('/crm/company/items', { company_name: `UI Buyer ${s}` })).data;
const supplier = (await admin.post('/crm/company/items', { company_name: `UI Supplier ${s}` })).data;
const inv = (await admin.post('/crm/invoice/items', { title: `UI invoice ${s}`, company_id: company.id, currency: base })).data;
await admin.put(`/crm/items/${inv.id}/products`, { rows: [{ name: 'Consulting', price: '200', quantity: '1', tax_rate: '10' }] });

const browser = await chromium.launch({ executablePath: exe });
const errors: string[] = [];
const page = await (await browser.newContext({ viewport: { width: 1440, height: 1000 } })).newPage();
page.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
page.on('console', (m) => m.type() === 'error' && !/401|400|404|409|Failed to load resource/.test(m.text()) && errors.push(m.text()));
page.on('dialog', (d) => d.accept());
let n = 0, failed = 0;
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); await page.screenshot({ path: `fail42-${++n}.png`, fullPage: true }); }
};
const pickParty = async (name: string, scope = page.locator('body')) => {
  await scope.locator('input[aria-label="Customer"], input[aria-label="Supplier"], input[aria-label="Customer or supplier"]').first().fill(name);
  await page.locator(`li button:has-text("${name}")`).first().click();
};
await page.goto(B + '/login'); await page.fill('#email', 'admin@acme.test'); await page.fill('#password', 'supersecret1');
await page.click('button:has-text("Sign in")'); await page.waitForURL(B + '/');

try {
  await step('the overview', async () => {
    await page.click('aside a:has-text("Overview")');
    await page.locator('[data-stat="receivable"]').waitFor();
  });

  await step('a CRM invoice posted to the books', async () => {
    await page.click('aside a:has-text("Sales invoices")');
    await page.click('[role="tab"]:has-text("To post")');
    await page.locator(`tr:has-text("UI invoice ${s}") button:has-text("Post")`).click();
    await page.click('dialog button:has-text("Post to the books")');
    await page.click('[role="tab"]:has-text("Open")');
    await page.locator(`tr:has-text("UI invoice ${s}") >> text=Open`).waitFor();
  });

  await step('a payment received', async () => {
    await page.locator(`tr:has-text("UI invoice ${s}") button:has-text("Receive payment")`).click();
    await page.selectOption('#pay-account', bank);
    // The amount fills in once the customer's open invoices have loaded.
    await page.waitForFunction(() => (document.querySelector('#pay-amount') as HTMLInputElement | null)?.value === '220.00', null, { timeout: 10_000 });
    await page.click('dialog button:has-text("Record payment received")');
    await page.click('[role="tab"]:has-text("Paid")');
    await page.locator(`tr:has-text("UI invoice ${s}")`).waitFor();
  });

  await step('the CRM invoice shows it', async () => {
    await page.goto(`${B}/crm/invoice/${inv.id}`);
    await page.locator('[data-books] >> text=Paid').first().waitFor();
    await page.locator('text=posted it to the books').first().waitFor();
  });
  await page.screenshot({ path: '63-invoice-in-books.png', fullPage: true });

  await step('a supplier\'s bill', async () => {
    await page.goto(`${B}/accounting/bills/new`);
    await pickParty(`UI Supplier ${s}`);
    await page.fill('#b-ref', `SUP-${s}`);
    await page.fill('input[aria-label="Line 1 description"]', 'Printer paper');
    await page.selectOption('select[aria-label="Line 1 account"]', costs);
    await page.fill('input[aria-label="Line 1 price"]', '45.50');
    await page.locator('[data-total]:has-text("45.50")').waitFor();
    await page.click('button:has-text("Post")');
    await page.locator('h1:has-text("Bill ")').waitFor();
    await page.locator('text=open').first().waitFor();
  });
  await page.screenshot({ path: '64-bill.png', fullPage: true });

  await step('a journal entry: balanced, posted, reversed', async () => {
    await page.goto(`${B}/accounting/journal/new`);
    await page.fill('#je-memo', `UI entry ${s}`);
    await page.selectOption('select[aria-label="Line 1 account"]', costs);
    await page.fill('input[aria-label="Line 1 debit"]', '30');
    await page.selectOption('select[aria-label="Line 2 account"]', bank);
    await page.fill('input[aria-label="Line 2 credit"]', '25');
    await page.locator('[data-difference]:has-text("Out of balance")').waitFor();
    if (await page.isEnabled('button:has-text("Post")')) throw new Error('posting an unbalanced entry should be off');
    await page.fill('input[aria-label="Line 2 credit"]', '30');
    await page.locator('[data-difference]:has-text("Balanced")').waitFor();
    await page.click('button:has-text("Post")');
    await page.locator('h1:has-text("Entry ")').waitFor();
    await page.click('button:has-text("Reverse")');
    await page.locator('[data-entry-status="void"]').waitFor();
    await page.locator('text=reversed by entry').waitFor();
  });

  await step('a bank statement: imported, a line recorded', async () => {
    await page.goto(`${B}/accounting/banking?account=${bank}`);
    const today = new Date().toISOString().slice(0, 10);
    await page.setInputFiles('input[aria-label="Statement file"]', { name: `stmt-${s}.csv`, mimeType: 'text/csv', buffer: Buffer.from(`Date,Description,Amount\n${today},UI bank fee ${s},-4.20\n${today},UI customer ${s},220.00\n`) });
    await page.locator(`[data-bank-line="UI bank fee ${s}"]`).waitFor();
    await page.locator(`[data-bank-line="UI customer ${s}"] button:has-text("Match entry")`).click();
    await page.locator(`[data-bank-line="UI bank fee ${s}"] button:has-text("Record")`).click();
    await page.selectOption('#rec-account', costs);
    await page.click('dialog button:has-text("Record")');
    await page.locator(`[data-bank-line="UI bank fee ${s}"]`).waitFor({ state: 'detached' });
  });
  await page.screenshot({ path: '65-banking.png', fullPage: true });

  await step('reports: profit and loss, a balanced balance sheet', async () => {
    await page.goto(`${B}/accounting/reports?tab=pl`);
    await page.locator(`[data-report="pl"] >> text=UI costs ${s}`).waitFor();
    await page.click('[role="tab"]:has-text("Balance sheet")');
    await page.locator('[data-balanced="true"]').waitFor();
    await page.click('[role="tab"]:has-text("Ledger")');
    await page.selectOption('#r-acc', bank);
    await page.locator('[data-closing]').waitFor();
  });
  await page.screenshot({ path: '66-reports.png', fullPage: true });
} finally {
  await browser.close();
  await cleanupRun(s);
}
console.log(errors.length ? 'console errors:\n' + errors.join('\n') : 'no console errors');
process.exit(failed || errors.length ? 1 : 0);
