// Debug helper: signs in as an existing guest and lists every request answered 403 while
// visiting the pages a guest uses. Usage: node guest-403-probe.ts <email> <password>
import { chromium } from 'playwright';
const B = 'http://localhost:8080';
const [email, password] = process.argv.slice(2);
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const p = await (await browser.newContext()).newPage();
p.on('response', (r) => r.status() === 403 && console.log('403', r.request().method(), r.url().replace(B, '')));
p.on('pageerror', (e) => console.log('PAGEERROR', e.message));
await p.goto(B + '/login'); await p.fill('#email', email); await p.fill('#password', password);
await p.click('button:has-text("Sign in")'); await p.waitForTimeout(2500);
for (const path of ['/profile', '/messenger', '/projects', '/tasks', '/boards']) {
  console.log('--', path);
  await p.goto(B + path); await p.waitForTimeout(2500);
}
await browser.close();
