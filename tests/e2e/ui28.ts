// Calendar in the browser: make an event with a guest, see it in month and week views,
// the guest answers, get a subscription link.
import { chromium } from 'playwright';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
const B = 'http://localhost:8080';
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: string[] = [];
const watch = (p: import('playwright').Page) => {
  p.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
  p.on('console', (m) => m.type() === 'error' && !/401|400|404|Failed to load resource/.test(m.text()) && errors.push(m.text()));
  p.on('dialog', (d) => d.accept());
};
const s = Date.now().toString(36);
let n = 0, failed = 0;
const pages: import('playwright').Page[] = [];
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `fail28-${n}-${i}.png`, fullPage: true }); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
await admin.post('/users', { email: `gil.${s}@acme.test`, password: 'password123', first_name: 'Gil', last_name: `Guest${s}` });
const login = async (email: string, password: string) => {
  const p = await (await browser.newContext({ viewport: { width: 1440, height: 1000 } })).newPage();
  watch(p);
  pages.push(p);
  await p.goto(B + '/login'); await p.fill('#email', email); await p.fill('#password', password);
  await p.click('button:has-text("Sign in")'); await p.waitForURL(B + '/');
  return p;
};
const page = await login('admin@acme.test', 'supersecret1');

try {
  await step('make an event with a guest', async () => {
    await page.click('aside a:has-text("Calendar")');
    await page.click('button:has-text("Month")');
    await page.click('button:has-text("New event")');
    await page.fill('#ev-title', `Planning ${s}`);
    await page.fill('#ev-loc', 'Room 7');
    await page.locator('dialog[open] input[placeholder="Invite colleagues…"]').fill(`Guest${s}`);
    await page.locator(`dialog[open] button:has-text("Gil Guest${s}")`).click();
    await page.click('dialog[open] button:has-text("Save")');
    await page.locator(`[data-entry="Planning ${s}"]`).first().waitFor();
  });

  await step('week view', async () => {
    await page.click('button:has-text("Week")');
    await page.locator(`[data-entry="Planning ${s}"]`).first().waitFor();
    await page.locator(`[data-entry="Planning ${s}"]`).first().click();
    await page.locator('#ev-title').waitFor();
    if ((await page.inputValue('#ev-loc')) !== 'Room 7') throw new Error('location not loaded');
    await page.click('dialog[open] button:has-text("Cancel")');
  });
  await page.screenshot({ path: '48-calendar-week.png' });

  await step('the guest answers', async () => {
    const gil = await login(`gil.${s}@acme.test`, 'password123');
    await gil.click('aside a:has-text("Calendar")');
    await gil.click('button:has-text("Week")');
    await gil.locator(`[data-entry="Planning ${s}"]`).first().click();
    await gil.locator('[data-event-view] >> text=Room 7').waitFor();
    await gil.click('dialog[open] button:has-text("Going")');
    await gil.locator('dialog[open] button[aria-pressed="true"]:has-text("Going")').waitFor();
  });

  await step('subscription link', async () => {
    await page.click('button:has-text("Subscribe")');
    // A link left over from an interrupted run is replaced ("Make a new link").
    await page.click('dialog[open] button:has-text("Make my link"), dialog[open] button:has-text("Make a new link")');
    const url = await page.locator('[data-feed]').textContent();
    const ics = await (await fetch(url!.replace(/^https?:\/\/[^/]+/, B))).text();
    if (!ics.includes(`SUMMARY:Planning ${s}`)) throw new Error('feed missing the event');
    await page.click('dialog[open] button:has-text("Turn the link off")');
  });
} finally {
  await cleanupRun(s);
}
console.log(errors.length ? 'console errors:\n' + errors.join('\n') : 'no console errors');
await browser.close();
process.exit(failed || errors.length ? 1 : 0);
