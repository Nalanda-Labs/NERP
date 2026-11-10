// Drive in the browser: a folder, uploads, a new version, a public link, the trash.
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
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); await page.screenshot({ path: `fail36-${++n}.png`, fullPage: true }); }
};
await page.goto(B + '/login'); await page.fill('#email', 'admin@acme.test'); await page.fill('#password', 'supersecret1');
await page.click('button:has-text("Sign in")'); await page.waitForURL(B + '/');
const file = (text: string) => ({ name: `report-${s}.txt`, mimeType: 'text/plain', buffer: Buffer.from(text) });

try {
  await step('a folder and an upload', async () => {
    await page.click('aside a:has-text("Drive")');
    answers.push(`Reports ${s}`);
    await page.click('button:has-text("New folder")');
    await page.click(`[data-node="Reports ${s}"]`);
    await page.locator(`nav[aria-label="Folders"] >> text=Reports ${s}`).waitFor();
    await page.setInputFiles('input[aria-label="Upload files"]', file('first'));
    await page.locator(`[data-node="report-${s}.txt"]`).waitFor();
  });

  await step('same name: a new version', async () => {
    await page.setInputFiles('input[aria-label="Upload files"]', file('second'));
    await page.locator(`[data-node="report-${s}.txt"] >> text=v2`).waitFor();
    await page.click(`[data-node="report-${s}.txt"]`);
    await page.locator('[data-detail] [data-version="1"]').waitFor();
  });

  await step('a public link', async () => {
    await page.fill('input[aria-label="Days the link works"]', '3');
    await page.click('button:has-text("Make a link")');
    const url = (await page.locator('[data-link]').textContent())!;
    const visitor = await browser.newPage();
    await visitor.goto(url.replace(/^https?:\/\/[^/]+/, B));
    await visitor.locator(`[data-file-name]:text-is("report-${s}.txt")`).waitFor();
    await visitor.close();
  });
  await page.screenshot({ path: '56-drive.png' });

  await step('the trash', async () => {
    await page.click('dialog[open] button:has-text("Delete")');
    await page.locator(`[data-node="report-${s}.txt"]`).waitFor({ state: 'detached' });
    await page.click('[role="tab"]:has-text("Trash")');
    await page.locator(`[data-trash="report-${s}.txt"] button:has-text("Put back")`).click();
    await page.locator(`[data-trash="report-${s}.txt"]`).waitFor({ state: 'detached' });
  });
} finally {
  await cleanupRun(s);
}
console.log(errors.length ? 'console errors:\n' + errors.join('\n') : 'no console errors');
await browser.close();
process.exit(failed || errors.length ? 1 : 0);
