// Website builder in the browser: make a site, edit the home page with the preview, add a
// block, publish, and see it as a visitor.
import { chromium } from 'playwright';
import { cleanupRun } from './cleanup.ts';
const B = 'http://localhost:8080';
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: string[] = [];
const ctx = await browser.newContext({ viewport: { width: 1500, height: 1000 } });
const page = await ctx.newPage();
page.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
page.on('console', (m) => m.type() === 'error' && !/401|400|404|Failed to load resource/.test(m.text()) && errors.push(m.text()));
page.on('dialog', (d) => d.accept());
const s = Date.now().toString(36);
let n = 0, failed = 0;
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); await page.screenshot({ path: `fail33-${++n}.png`, fullPage: true }); }
};
await page.goto(B + '/login'); await page.fill('#email', 'admin@acme.test'); await page.fill('#password', 'supersecret1');
await page.click('button:has-text("Sign in")'); await page.waitForURL(B + '/');

try {
  await step('make a website', async () => {
    await page.click('aside a:has-text("Websites")');
    await page.click('button:has-text("New website")');
    await page.fill('#st-n', `Bakery ${s}`);
    await page.click('dialog[open] button:has-text("Create")');
    await page.waitForURL(/\/sites\/[0-9a-f-]{36}$/);
    await page.locator('[data-page="Home"]').click();
  });

  await step('edit with a live preview', async () => {
    const hero = page.locator('[data-edit-block="hero"]');
    await hero.locator('input[aria-label="Heading"]').fill(`Fresh bread ${s}`);
    await page.locator(`[data-preview] h1:text-is("Fresh bread ${s}")`).waitFor();
    await page.selectOption('select[aria-label="Block to add"]', 'contact');
    await page.click('button:has-text("Add block")');
    await page.locator('[data-edit-block="contact"] input[placeholder="Phone"]').fill('+1 555 0199');
    await page.locator('[data-preview] [data-block="contact"] >> text=+1 555 0199').waitFor();
    await page.click('button:has-text("Publish")');
    await page.locator('[data-state]:text-is("Published")').waitFor();
  });

  await step('a visitor sees it', async () => {
    const visitor = await (await browser.newContext({ viewport: { width: 390, height: 844 } })).newPage();
    await visitor.goto(`${B}/s/bakery-${s}`);
    await visitor.locator(`h1:text-is("Fresh bread ${s}")`).waitFor();
    await visitor.locator('[data-block="contact"] a:text-is("+1 555 0199")').waitFor();
    await visitor.screenshot({ path: '53-website.png', fullPage: true });
    await visitor.close();
  });
} finally {
  await cleanupRun(s);
}
console.log(errors.length ? 'console errors:\n' + errors.join('\n') : 'no console errors');
await browser.close();
process.exit(failed || errors.length ? 1 : 0);
