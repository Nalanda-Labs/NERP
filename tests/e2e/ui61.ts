// BI in the browser: a dashboard made from deals (a column chart and a single figure compared
// with the month before), changing the period, sharing it with a colleague who sees it
// read-only, a public link opened by anyone, and a CSV file uploaded as a dataset.
import { chromium } from 'playwright';
import { writeFileSync } from 'node:fs';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
const B = 'http://localhost:8080';
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: string[] = [];
const s = Date.now().toString(36);
let n = 0, failed = 0;
const pages: import('playwright').Page[] = [];
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `fail61-${n}-${i}.png`, fullPage: true }); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
await admin.post('/users', { email: `zia.${s}@acme.test`, password: 'password123', first_name: 'Zia', last_name: `Bi${s}` });
const base = (await admin.get('/crm/config')).data.base_currency;
for (const [amount, source] of [[1000, 'website'], [500, 'website'], [250, 'call']] as const) {
  await admin.post('/crm/deal/items', { title: `Ui deal ${s} ${amount}`, currency: base, opportunity: String(amount), source });
}
const dashboards: string[] = [];
const datasets: string[] = [];

const ctx = async (email: string | null, password = 'password123') => {
  const p = await (await browser.newContext({ viewport: { width: 1440, height: 1000 }, timezoneId: 'UTC', locale: 'en-GB', acceptDownloads: true })).newPage();
  p.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
  p.on('console', (m) => m.type() === 'error' && !/401|400|404|409|Failed to load resource/.test(m.text()) && errors.push(m.text()));
  pages.push(p);
  if (email) {
    await p.goto(B + '/login'); await p.fill('#email', email); await p.fill('#password', password);
    await p.click('button:has-text("Sign in")'); await p.waitForURL(B + '/');
  }
  return p;
};

try {
  const ada = await ctx('admin@acme.test', 'supersecret1');
  let dashUrl = '';
  let link = '';

  await step('make a dashboard', async () => {
    await ada.goto(B + '/bi');
    await ada.click('button:has-text("New dashboard")');
    await ada.fill('#bn-title', `Sales ${s}`);
    await ada.click('dialog button:has-text("Create")');
    await ada.waitForURL(/\/bi\/[0-9a-f-]{36}\?edit=1/);
    dashUrl = ada.url().split('?')[0];
    dashboards.push(dashUrl.split('/').pop()!);
  });

  await step('a column chart of deals by source, with a live preview', async () => {
    await ada.click('button:has-text("+ Chart")');
    await ada.fill('#bw-title', 'By source');
    await ada.selectOption('#bw-ds', 'nerp:deals');
    await ada.selectOption('dialog select[aria-label="Group by"]', 'source');
    await ada.click('dialog button:has-text("+ Add a condition")');
    await ada.selectOption('dialog [data-filters] select[aria-label="Column"]', 'title');
    await ada.selectOption('dialog [data-filters] select[aria-label="Condition"]', 'contains');
    await ada.fill('dialog [data-filters] input[aria-label="Value"]', s);
    await ada.locator('dialog [aria-label="Preview"] [data-echart] canvas').waitFor();
    await ada.click('dialog button.btn-primary:text-is("Add")');
    await ada.locator('[data-widget="By source"] [data-echart] canvas').waitFor();
  });

  await step('a single figure, compared with the month before', async () => {
    await ada.click('button:has-text("+ Chart")');
    await ada.fill('#bw-title', 'Deal value');
    await ada.click('dialog button:has-text("Single figure")');
    await ada.selectOption('#bw-ds', 'nerp:deals');
    await ada.selectOption('dialog select[aria-label="Figure"]', 'sum');
    await ada.selectOption('dialog select[aria-label="Of column"]', 'amount');
    await ada.click('dialog button:has-text("+ Add a condition")');
    await ada.selectOption('dialog [data-filters] select[aria-label="Condition"]', 'contains');
    await ada.fill('dialog [data-filters] input[aria-label="Value"]', s);
    await ada.check('dialog label:has-text("Compare with the period before") input');
    await ada.locator('dialog [aria-label="Preview"] [data-figure]:has-text("1,750")').waitFor();
    await ada.click('dialog button.btn-primary:text-is("Add")');
    await ada.locator('[data-widget="Deal value"] [data-figure]:has-text("1,750")').waitFor();
    await ada.locator('[data-widget="Deal value"] [data-change]:has-text("before")').waitFor();
  });

  await step('make it wider, then done', async () => {
    await ada.click('button[aria-label="Move Deal value earlier"]');
    await ada.locator('[data-widget]').first().locator('h2:has-text("Deal value")').waitFor();
    await ada.click('[data-widget="Deal value"] button[aria-label="Wider"]');
    await ada.click('button:has-text("Done")');
    await ada.locator('button:has-text("Edit")').waitFor();
  });

  await step('last year shows nothing', async () => {
    await ada.selectOption('select[aria-label="Period"]', 'last_year');
    await ada.locator('[data-widget="Deal value"] [data-figure]:has-text("0")').waitFor();
    await ada.locator('[data-widget="By source"]:has-text("Nothing for this period")').waitFor();
    await ada.selectOption('select[aria-label="Period"]', 'this_month');
    await ada.locator('[data-widget="Deal value"] [data-figure]:has-text("1,750")').waitFor();
  });

  await step('share with Zia and make a public link', async () => {
    await ada.click('button:has-text("Share")');
    await ada.fill('dialog input[placeholder="Add a person…"]', `Zia Bi${s}`);
    await ada.click(`dialog button:has-text("Zia Bi${s}")`);
    await ada.click('dialog button:has-text("Save")');
    await ada.locator('text=Sharing saved').waitFor();
    await ada.fill('dialog input[aria-label="Link password"]', 'letmein');
    await ada.click('dialog button:has-text("Make a link")');
    link = (await ada.locator('[data-dashboard-link]').textContent())!.trim();
    await ada.screenshot({ path: '/tmp/nerp-e2e/bi-share.png' });
    await ada.click('dialog button:has-text("Close")');
    await ada.screenshot({ path: '/tmp/nerp-e2e/bi-dashboard.png', fullPage: true });
  });

  await step('Zia sees it, read-only', async () => {
    const zia = await ctx(`zia.${s}@acme.test`);
    await zia.goto(B + '/bi');
    await zia.click(`[data-dashboard="Sales ${s}"]`);
    await zia.locator('[data-widget="By source"] [data-echart] canvas').waitFor();
    await zia.locator('[data-widget="Deal value"] [data-figure]:has-text("1,750")').waitFor();
    if (await zia.locator('button:has-text("Edit")').count()) throw new Error('Zia may edit');
    if (await zia.locator('a:has-text("Datasets")').count()) throw new Error('Zia sees datasets');
  });

  await step('anyone with the link and its password', async () => {
    const anon = await ctx(null);
    await anon.goto(link);
    await anon.fill('input[aria-label="Password"]', 'letmein');
    await anon.click('button:has-text("Open")');
    await anon.locator(`h1:has-text("Sales ${s}")`).waitFor();
    await anon.locator('[data-widget="Deal value"] [data-figure]:has-text("1,750")').waitFor();
    await anon.locator('[data-widget="By source"] [data-echart] canvas').waitFor();
    await anon.screenshot({ path: '/tmp/nerp-e2e/bi-public.png', fullPage: true });
  });

  await step('upload a CSV as a dataset, look at it and download it', async () => {
    const f = `/tmp/nerp-e2e/regions-${s}.csv`;
    writeFileSync(f, 'Region,Sales,Day\nNorth,120.5,2026-01-05\nSouth,300,2026-02-01\nNorth,79.5,2026-02-03\n');
    await ada.goto(B + '/bi/datasets');
    await ada.click('button:has-text("Upload a file")');
    await ada.fill('#da-name', `Regions ${s}`);
    await ada.setInputFiles('#da-file', f);
    await ada.click('dialog button.btn-primary:text-is("Add")');
    await ada.locator('[data-preview] td:has-text("South")').waitFor();
    await ada.locator('text=3 rows').waitFor();
    datasets.push((await admin.get('/bi/datasets')).data.items.find((d: any) => d.name === `Regions ${s}`).id);
    const [dl] = await Promise.all([ada.waitForEvent('download'), ada.click('button:has-text("Download CSV")')]);
    if (!dl.suggestedFilename().endsWith('.csv')) throw new Error(dl.suggestedFilename());
    await ada.click(`[data-dataset="Deals"]`);
    await ada.locator('[data-preview]').waitFor();
    await ada.screenshot({ path: '/tmp/nerp-e2e/bi-datasets.png', fullPage: true });
  });

  await step('no browser errors', async () => {
    if (errors.length) throw new Error(errors.join(' | '));
  });
} finally {
  await browser.close();
  for (const id of dashboards) await admin.del(`/bi/dashboards/${id}`);
  for (const id of datasets) await admin.del(`/bi/datasets/${id}`);
  await cleanupRun(s);
}
console.log(failed ? `\n${failed} failed` : '\nall passed');
process.exit(failed ? 1 : 0);
