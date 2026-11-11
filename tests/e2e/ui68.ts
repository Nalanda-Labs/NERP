// The knowledge base's Markdown editor in the browser: formatting from the toolbar and
// shortcuts, a table, a link to another page by typing [[, a picture uploaded and shown,
// the live preview and its views, Ctrl+S, and the saved page with its contents, anchors,
// highlighted code and callout; leaving with unsaved changes asks first.
import { chromium } from 'playwright';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
const B = 'http://localhost:8080';
const s = Date.now().toString(36);
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: string[] = [];
let failed = 0, n = 0;
let p!: import('playwright').Page;
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); await p?.screenshot({ path: `fail68-${++n}.png`, fullPage: true }); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const space = (await admin.post('/kb/spaces', { name: `Editor ${s}` })).data;
const target = (await admin.post(`/kb/spaces/${space.id}/pages`, { title: `Holidays ${s}`, body: 'Days off.' })).data;
const page = (await admin.post(`/kb/spaces/${space.id}/pages`, { title: `Handbook ${s}`, body: '' })).data;
const PNG = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==', 'base64');

try {
  p = await (await browser.newContext({ viewport: { width: 1440, height: 950 }, timezoneId: 'UTC', locale: 'en-GB' })).newPage();
  p.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
  p.on('console', (m) => m.type() === 'error' && !/401|403|404|Failed to load resource/.test(m.text()) && errors.push(m.text()));
  await p.goto(B + '/login'); await p.fill('#email', 'admin@acme.test'); await p.fill('#password', 'supersecret1');
  await p.click('button:has-text("Sign in")'); await p.waitForURL(B + '/');
  const editor = () => p.locator('[aria-label="Page text (Markdown)"]');
  const preview = () => p.locator('[data-editor-preview]');
  const type = (t: string) => p.keyboard.type(t, { delay: 5 });

  await step('the editor opens with its toolbar, split into source and preview', async () => {
    await p.goto(B + `/kb/page/${page.id}?edit=1`);
    await editor().waitFor();
    await p.locator('[role="toolbar"][aria-label="Formatting"] button[aria-label="Bold"]').waitFor();
    await preview().waitFor();
  });

  await step('headings, bold and lists from the toolbar and shortcuts', async () => {
    await editor().click();
    await type('Welcome');
    await p.click('button[aria-label="Heading 1"]');
    await p.keyboard.press('End');
    await p.keyboard.press('Enter'); await p.keyboard.press('Enter');
    await p.keyboard.press('Control+b'); await type('important'); await p.keyboard.press('End');
    await p.keyboard.press('Enter'); await p.keyboard.press('Enter');
    await type('## Steps'); await p.keyboard.press('Enter'); await p.keyboard.press('Enter');
    await type('- first'); await p.keyboard.press('Enter'); await type('second');
    await preview().locator('h1:text("Welcome")').waitFor();
    await preview().locator('strong:text-is("important")').waitFor();
    await preview().locator('li:text-is("second")').waitFor(); // Enter continued the list
  });

  await step('a table and a callout', async () => {
    await p.keyboard.press('Enter'); await p.keyboard.press('Enter');
    await p.click('button[aria-label="Table"]');
    await preview().locator('table th:text-is("Column")').first().waitFor();
    await p.keyboard.press('Control+End');
    await p.keyboard.press('Enter');
    await p.click('button[title="Callout"]');
    await p.click('[role="menuitem"]:has-text("Warning")');
    await type('Mind the gap');
    await preview().locator('blockquote.markdown-alert-warning:has-text("Mind the gap")').waitFor();
    // Enter keeps the quote going (as it should); a blank line leaves it.
    await p.keyboard.insertText('\n\n');
  });

  await step('[[ links another page', async () => {
    await p.keyboard.press('Enter'); await p.keyboard.press('Enter');
    await type(`See [[Holid`);
    await p.locator('.cm-tooltip-autocomplete li', { hasText: `Holidays ${s}` }).click();
    await preview().locator(`a[href="/kb/page/${target.id}"]:text("Holidays ${s}")`).waitFor();
    const src = await editor().innerText();
    if (src.includes(']]')) throw new Error('leftover ]] after the link');
    if (await preview().locator('blockquote:has-text("See")').count()) throw new Error('the link ended up in the callout');
  });

  await step('code is highlighted in the preview', async () => {
    await p.keyboard.press('Enter'); await p.keyboard.press('Enter');
    await type('### Query'); await p.keyboard.press('Enter'); await p.keyboard.press('Enter');
    await p.keyboard.insertText('```sql\nSELECT name FROM people;\n```\n');
    await preview().locator('pre code.hljs .hljs-keyword').first().waitFor();
  });

  await step('a picture is uploaded and shown', async () => {
    await p.keyboard.press('Control+End');
    await p.locator('[data-markdown-editor] ~ input[type="file"]').setInputFiles({ name: 'logo.png', mimeType: 'image/png', buffer: PNG });
    await preview().locator('img[alt="logo.png"][src*="dl="]').waitFor({ timeout: 15000 });
    const ok = await preview().locator('img[alt="logo.png"]').evaluate((i: HTMLImageElement) => new Promise((r) => (i.complete ? r(i.naturalWidth > 0) : (i.onload = () => r(true), i.onerror = () => r(false)))));
    if (!ok) throw new Error('the picture did not load');
  });

  await step('views: write only, preview only, back to split', async () => {
    await p.click('[role="radio"]:has-text("Write")');
    if (await preview().count()) throw new Error('preview still shown');
    await p.click('[role="radio"]:has-text("Preview")');
    await preview().locator('h1:text("Welcome")').waitFor();
    if (await editor().isVisible()) throw new Error('source still shown');
    await p.click('[role="radio"]:has-text("Split")');
    await editor().waitFor();
  });

  await step('the editor takes the full width (no page tree beside it)', async () => {
    if (await p.locator('aside[aria-label="Pages"]').count()) throw new Error('the page tree is still shown');
    const ed = await p.locator('[data-markdown-editor]').boundingBox();
    const main = await p.locator('[data-markdown-editor]').evaluate((e) => e.parentElement!.parentElement!.getBoundingClientRect().width);
    if (!ed || ed.width < main - 4) throw new Error(`editor ${ed?.width} of ${main}`);
  });

  await step('a reload keeps the editor and the unsaved text', async () => {
    if (!p.url().includes('edit=1')) throw new Error(`address: ${p.url()}`);
    await p.reload();
    await editor().waitFor();
    await preview().locator('blockquote.markdown-alert-warning:has-text("Mind the gap")').waitFor();
    await p.locator('text=Your unsaved changes are back').waitFor();
  });

  await step('leaving with unsaved changes asks first', async () => {
    let asked = '';
    p.once('dialog', (d) => { asked = d.message(); d.dismiss(); });
    await p.click('button:has-text("Cancel")');
    if (!/Throw away/.test(asked)) throw new Error(`asked: ${asked}`);
    await editor().waitFor();
  });

  await step('Ctrl+S saves', async () => {
    await editor().click();
    await p.keyboard.press('Control+s');
    await p.locator('[data-page-body]').waitFor();
    const saved = (await admin.get(`/kb/pages/${page.id}`)).data;
    if (!saved.body.includes('**important**') || !saved.body.includes(`(/kb/page/${target.id})`) || !/!\[logo\.png\]\(\/api\/files\/[0-9a-f-]+\/content\)/.test(saved.body)) throw new Error(saved.body);
  });

  await step('the saved page: contents, anchors, highlighted code, the picture', async () => {
    const body = p.locator('[data-page-body]');
    await p.locator('[data-toc] a:text("Steps")').waitFor();
    await body.locator('h2#h-steps .anchor').waitFor({ state: 'attached' });
    await body.locator('pre code.hljs .hljs-keyword').first().waitFor();
    await body.locator('img[alt="logo.png"][src*="dl="]').waitFor();
    await p.click('[data-toc] a:text("Query")');
    await p.waitForFunction(() => location.hash === '#h-query' || document.getElementById('h-query')!.getBoundingClientRect().top < 300);
  });

  await step('no browser errors', async () => { if (errors.length) throw new Error(errors.join(' | ')); });
} finally {
  await browser.close();
  await admin.del(`/kb/spaces/${space.id}`);
  await cleanupRun(s);
}
console.log(failed ? `\n${failed} failed` : '\nall passed');
process.exit(failed ? 1 : 0);
