// Knowledge base in the browser: a space, a page written in Markdown with a live preview,
// a second version, history and restoring, search.
import { chromium } from 'playwright';
import { cleanupRun } from './cleanup.ts';
const B = 'http://localhost:8080';
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: string[] = [];
const page = await (await browser.newContext({ viewport: { width: 1440, height: 1000 } })).newPage();
page.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
page.on('console', (m) => m.type() === 'error' && !/401|400|404|Failed to load resource/.test(m.text()) && errors.push(m.text()));
const answers: string[] = [];
page.on('dialog', (d) => d.accept(d.type() === 'prompt' ? answers.shift() : undefined));
const s = Date.now().toString(36);
let n = 0, failed = 0;
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); await page.screenshot({ path: `fail31-${++n}.png`, fullPage: true }); }
};
await page.goto(B + '/login'); await page.fill('#email', 'admin@acme.test'); await page.fill('#password', 'supersecret1');
await page.click('button:has-text("Sign in")'); await page.waitForURL(B + '/');

try {
  await step('a space', async () => {
    await page.click('aside a:has-text("Knowledge base")');
    await page.click('button:has-text("New space")');
    await page.fill('#kb-name', `Playbook ${s}`);
    await page.click('dialog[open] button:has-text("Create")');
    await page.locator(`h1:text-is("Playbook ${s}")`).waitFor();
  });

  await step('a page in Markdown, with a preview', async () => {
    answers.push('Onboarding');
    await page.click('button:has-text("New page")');
    await page.waitForURL(/\/kb\/page\/.+\?edit=1/);
    await page.fill('textarea[aria-label="Page text (Markdown)"]', `## First day ${s}\n\n- Laptop\n- **Badge**`);
    await page.locator('[aria-label="Preview"] h2', { hasText: `First day ${s}` }).waitFor();
    await page.locator('[aria-label="Preview"] strong:text-is("Badge")').waitFor();
    await page.click('button:has-text("Save")');
    await page.locator('[data-page-body] li:text-is("Laptop")').waitFor();
    await page.locator('text=Version 2').waitFor();
  });

  await step('history and restoring', async () => {
    await page.click('button:has-text("Edit")');
    await page.fill('textarea[aria-label="Page text (Markdown)"]', `Replaced ${s}`);
    await page.click('button:has-text("Save")');
    await page.locator('text=Version 3').waitFor();
    await page.click('button:has-text("History")');
    await page.locator('[data-version="2"] button:has-text("Restore")').click();
    await page.locator('[data-page-body] li:text-is("Laptop")').waitFor();
    await page.locator('text=Version 4').waitFor();
  });
  await page.screenshot({ path: '51-kb-page.png' });

  await step('search', async () => {
    await page.click('aside a:has-text("Knowledge base")');
    await page.fill('input[aria-label="Search all pages"]', `First day ${s}`);
    await page.locator('[data-hit="Onboarding"]').click();
    await page.locator('h1:text-is("Onboarding")').waitFor();
  });
} finally {
  await cleanupRun(s);
}
console.log(errors.length ? 'console errors:\n' + errors.join('\n') : 'no console errors');
await browser.close();
process.exit(failed || errors.length ? 1 : 0);
