import '../output.ts'; // screenshots go outside the repository
import { chromium } from 'playwright';
import fs from 'node:fs';
const B = 'http://localhost:8080';
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const page = await (await browser.newContext({ acceptDownloads: true, viewport: { width: 1440, height: 1000 } })).newPage();
await page.goto(B + '/login'); await page.fill('#email', 'admin@acme.test'); await page.fill('#password', 'supersecret1');
await page.click('button:has-text("Sign in")'); await page.waitForURL(B + '/');
await page.goto(B + '/crm/settings?tab=documents');
await page.click('aside >> button:has-text("Invoice · Modern (Word)")');
await page.click('button:has-text("Preview")');
await page.waitForSelector('iframe[title="PDF preview"]', { timeout: 60000 });
const [dl] = await Promise.all([page.waitForEvent('download'), page.click('a:has-text("Download PDF")')]);
const b = fs.readFileSync(await dl.path());
console.log(dl.suggestedFilename(), b.subarray(0, 5).toString(), b.length);
await browser.close();
