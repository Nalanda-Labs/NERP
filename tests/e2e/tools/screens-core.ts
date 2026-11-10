import '../output.ts'; // screenshots go outside the repository
import { chromium } from 'playwright';
const B = 'http://localhost:8080';
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const page = await (await browser.newContext({ viewport: { width: 1440, height: 900 } })).newPage();
await page.goto(B + '/login'); await page.waitForTimeout(500); await page.screenshot({ path: 'g-login.png' });
await page.fill('#email', 'admin@acme.test'); await page.fill('#password', 'supersecret1'); await page.click('button:has-text("Sign in")');
await page.waitForURL(B + '/'); await page.waitForTimeout(1200); await page.screenshot({ path: 'g-dashboard.png' });
await page.goto(B + '/tasks?layout=board'); await page.waitForTimeout(1500); await page.screenshot({ path: 'g-tasks.png' });
await browser.close();
