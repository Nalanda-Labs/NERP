// CRM → Documents page and the bookmarkable CRM settings tab.
import { chromium } from 'playwright';
import fs from 'node:fs';
import { cleanupRun } from './cleanup.ts';
const B = 'http://localhost:8080';
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: any[] = [];
const ctx = await browser.newContext({ viewport: { width: 1440, height: 1000 }, acceptDownloads: true });
const page = await ctx.newPage();
page.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
page.on('console', (m) => m.type() === 'error' && !/401|400|Failed to load resource/.test(m.text()) && errors.push(m.text()));
let n = 0, failed = 0;
const step = async (name, fn) => { try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); await page.screenshot({ path: `fail13-${++n}.png`, fullPage: true }); } };
const s = Date.now().toString(36);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const api = async (method: string, path: string, body?: any, token?: string | null) => (await fetch(B + '/api' + path, { method, headers: { 'content-type': 'application/json', ...(token ? { authorization: `Bearer ${token}` } : {}) }, body: body ? JSON.stringify(body) : undefined })).json();
const token = (await api('POST', '/auth/login', { email: 'admin@acme.test', password: 'supersecret1' })).access_token;

// Data: a deal with three invoices.
const company = (await api('POST', '/crm/company/items', { company_name: `Pdf Co ${s}` }, token)).id;
const deal = (await api('POST', '/crm/deal/items', { title: `Pdf deal ${s}`, company_id: company, currency: 'EUR', opportunity: '990' }, token)).id;
const tpls = (await api('GET', '/crm/doc-templates', null, token)).items;
const docs: any[] = [];
for (const k of ['typst.invoice.classic', 'typst.invoice.modern', 'typst.invoice.minimal']) {
  let d = await api('POST', `/crm/items/${deal}/documents`, { template_id: tpls.find((t) => t.builtin_key === k).id }, token);
  for (let i = 0; i < 60 && d.status === 'pending'; i++) { await sleep(300); d = await api('GET', `/crm/documents/${d.id}`, null, token); }
  docs.push(d);
}
// A quote on another deal, so "Quote" is among the kinds (the filter lists kinds that exist).
const other = (await api('POST', '/crm/deal/items', { title: `Quote deal ${s}`, currency: 'EUR' }, token)).id;
await api('POST', `/crm/items/${other}/documents`, { template_id: tpls.find((t) => t.builtin_key === 'typst.quote').id }, token);

await page.goto(B + '/login'); await page.fill('#email', 'admin@acme.test'); await page.fill('#password', 'supersecret1');
await page.click('button:has-text("Sign in")'); await page.waitForURL(B + '/');

await step('settings tab is in the address', async () => {
  await page.goto(B + '/crm/settings?tab=documents');
  await page.locator('a[aria-current="page"]:text-is("Documents")').waitFor();
  await page.locator('text=Invoice · Modern (Typst)').first().waitFor();
  await page.click('a[href="?tab=automation"]');
  await page.waitForURL(/tab=automation/);
  await page.goBack().catch(() => {});
});

await step('download and reopen template files', async () => {
  const A = new URL('../../backend/assets/doc-templates/', import.meta.url).pathname;
  await page.goto(B + '/crm/settings?tab=documents');
  for (const [name, file, ext] of [['Invoice · Modern (Typst)', 'invoice-modern.typ', 'typ'], ['Invoice · Classic (LaTeX)', 'invoice-classic.tex', 'tex'], ['Invoice · Minimal (Word)', 'invoice-minimal.docx', 'docx']]) {
    await page.click(`aside >> button:has-text("${name}")`);
    const btn = ext === 'docx' ? 'button:has-text("Download current")' : 'button[title="Download the source as a file"]';
    const [dl] = await Promise.all([page.waitForEvent('download'), page.click(btn)]);
    if (dl.suggestedFilename() !== `${name}.${ext}`) throw new Error('name ' + dl.suggestedFilename());
    if (!fs.readFileSync(await dl.path()).equals(fs.readFileSync(A + file))) throw new Error('content differs: ' + file);
  }
  // The rendered preview downloads as a PDF.
  await page.click('aside >> button:has-text("Invoice · Classic (Typst)")');
  await page.waitForSelector('iframe[title="PDF preview"]');
  const [pdf] = await Promise.all([page.waitForEvent('download'), page.click('a:has-text("Download PDF")')]);
  if (pdf.suggestedFilename() !== 'Invoice · Classic (Typst).pdf') throw new Error('pdf name ' + pdf.suggestedFilename());
  if (!fs.readFileSync(await pdf.path()).subarray(0, 5).toString().startsWith('%PDF-')) throw new Error('preview is not a PDF');
  // Open a .typ file into the editor (not saved).
  await page.click('aside >> button:has-text("Invoice · Minimal (Typst)")');
  const tmp = 'upload-test.typ';
  fs.writeFileSync(tmp, '#let d = json("data.json")\nHello from a file ' + s);
  page.once('dialog', (d) => d.accept());
  await page.setInputFiles('input[type=file][accept^=".typ"]', tmp);
  await page.waitForFunction((v) => document.querySelector<HTMLInputElement>('textarea[aria-label="Template source"]')!.value.includes(v), 'Hello from a file ' + s);
  await page.waitForSelector('iframe[title="PDF preview"]');
  fs.unlinkSync(tmp);
});

await step('fill a template preview with a real deal', async () => {
  await page.goto(B + '/crm/settings?tab=documents');
  await page.click('aside >> button:has-text("Invoice · Modern (Typst)")');
  await page.waitForSelector('iframe[title="PDF preview"]');
  await page.fill('input[placeholder="Find a deal, lead…"]', `Pdf deal ${s}`);
  await page.click(`ul >> button:has-text("Pdf deal ${s}")`);
  await page.locator(`h3:has-text("Preview · Pdf deal ${s}")`).waitFor();
  await sleep(300);
  await page.waitForFunction(() => !document.body.innerText.includes('Rendering…'));
  const [dl] = await Promise.all([page.waitForEvent('download'), page.click('a:has-text("Download PDF")')]);
  const path = await dl.path();
  const { execSync } = await import('node:child_process');
  const text = execSync(`pdftotext "${path}" -`).toString();
  if (!text.includes(`Pdf Co ${s}`) || !text.includes('990')) throw new Error('real data missing: ' + text.slice(0, 300));
  await page.click('button:has-text("Use sample data")');
  await page.locator('h3:has-text("Preview · sample data")').waitFor();
});

await step('Documents in the sidebar opens the list', async () => {
  await page.goto(B + '/');
  await page.click('aside >> a:has-text("Documents")');
  await page.waitForURL(B + '/crm/documents');
  await page.locator('h1:text-is("Documents")').waitFor();
});

await step('search filters and goes into the address', async () => {
  await page.fill('#d-q', `Pdf deal ${s}`);
  await page.waitForURL(new RegExp(`q=Pdf`));
  await page.locator(`tbody tr:has-text("${docs[0].number}")`).waitFor();
  let rows = 0;
  for (let i = 0; i < 40 && (rows = await page.locator('tbody tr').count()) !== 3; i++) await sleep(100);
  if (rows !== 3) throw new Error(`rows ${rows}`);
  await page.locator(`tbody >> text=Pdf Co ${s}`).first().waitFor();
  await page.selectOption('#d-k', 'Quote');
  await page.locator('text=No documents match these filters.').waitFor();
  await page.selectOption('#d-k', 'Invoice');
  await page.locator(`tbody tr:has-text("${docs[1].number}")`).waitFor();
  await page.screenshot({ path: '27-documents.png', fullPage: true });
});

await step('filtered address can be reopened', async () => {
  await page.reload();
  await page.locator(`tbody tr:has-text("${docs[2].number}")`).waitFor();
  if ((await page.inputValue('#d-q')) !== `Pdf deal ${s}` || (await page.inputValue('#d-k')) !== 'Invoice') throw new Error('filters not restored');
});

await step('download one PDF', async () => {
  const [dl] = await Promise.all([page.waitForEvent('download'), page.click(`button[aria-label="Download ${docs[0].number}"]`)]);
  if (dl.suggestedFilename() !== `${docs[0].number}.pdf`) throw new Error(dl.suggestedFilename());
  const p = await dl.path();
  if (!fs.readFileSync(p).subarray(0, 5).toString().startsWith('%PDF-')) throw new Error('not a PDF');
});

await step('select all and download a ZIP', async () => {
  await page.check('thead input[type=checkbox]');
  await page.locator('button:has-text("Download 3 PDFs (ZIP)")').waitFor();
  const [dl] = await Promise.all([page.waitForEvent('download'), page.click('button:has-text("Download 3 PDFs (ZIP)")')]);
  if (!/^documents-.*\.zip$/.test(dl.suggestedFilename())) throw new Error(dl.suggestedFilename());
  const buf = fs.readFileSync(await dl.path());
  if (buf.subarray(0, 2).toString() !== 'PK' || !docs.every((d) => buf.includes(Buffer.from(`${d.number}.pdf`)))) throw new Error('zip content');
});

await step('record link opens the deal', async () => {
  await page.click(`tbody >> a:text-is("Pdf deal ${s}")`);
  await page.waitForURL(new RegExp(`/crm/deal/${deal}`));
});

await page.goto(B + '/');
await api('DELETE', `/crm/deal/items/${deal}`, null, token);
await api('DELETE', `/crm/company/items/${company}`, null, token);
console.log(errors.length ? 'console errors:\n' + errors.join('\n') : 'no console errors');
await browser.close();
await cleanupRun(s);
process.exit(failed || errors.length ? 1 : 0);
