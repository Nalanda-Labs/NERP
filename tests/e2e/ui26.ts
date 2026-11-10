// Live chat in the browser: set up a chat button, a visitor chats through its page, the
// operator answers from Chats (live), ends it, and the transcript is on the lead.
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
const page = await (await browser.newContext({ viewport: { width: 1440, height: 1000 } })).newPage();
watch(page);
const s = Date.now().toString(36);
let n = 0, failed = 0;
const pages = [page];
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `fail26-${n}-${i}.png`, fullPage: true }); }
};

await page.goto(B + '/login'); await page.fill('#email', 'admin@acme.test'); await page.fill('#password', 'supersecret1');
await page.click('button:has-text("Sign in")'); await page.waitForURL(B + '/');
const visitor = await (await browser.newContext({ viewport: { width: 380, height: 560 } })).newPage();
watch(visitor);
pages.push(visitor);
let key = '';

try {
  await step('set up a chat button', async () => {
    await page.click('aside a:has-text("Chat buttons")');
    await page.click('button:has-text("New chat button")');
    await page.fill('#cf-name', `Website ${s}`);
    await page.fill('#cf-greet', `Welcome ${s}!`);
    await page.click('dialog[open] button:has-text("Save")');
    const code = await page.locator(`[data-channel="Website ${s}"] [data-code]`).textContent();
    key = code!.match(/data-chat="([^"]+)"/)![1];
    if (!code!.includes('/chat.js')) throw new Error('no script code');
  });

  await step('a visitor writes', async () => {
    await visitor.goto(`${B}/c/${key}?u=${encodeURIComponent('https://acme.example/')}`);
    await visitor.locator(`text=Welcome ${s}!`).waitFor();
    await visitor.fill('#ch-name', `Ivy Stone ${s}`);
    await visitor.fill('#ch-email', `ivy.${s}@example.com`);
    await visitor.click('button:has-text("Start chat")');
    await visitor.fill('textarea[aria-label="Message"]', 'Is the blue one in stock?');
    await visitor.press('textarea[aria-label="Message"]', 'Enter');
    await visitor.locator('[data-msg="visitor"] >> text=Is the blue one in stock?').waitFor();
  });

  await step('the operator answers live', async () => {
    await page.click('aside a:has-text("Chats")');
    const item = page.locator(`[data-chat="Ivy Stone ${s}"]`);
    await item.waitFor();
    await item.click();
    await page.locator('[data-conversation] >> text=Is the blue one in stock?').waitFor();
    await page.locator('[data-crm-link]').filter({ hasText: `Chat: Ivy Stone ${s}` }).waitFor();
    await page.fill('textarea[aria-label="Reply"]', 'Yes, three left.');
    await page.press('textarea[aria-label="Reply"]', 'Enter');
    await visitor.locator('[data-msg="operator"] >> text=Yes, three left.').waitFor({ timeout: 10000 });
    await visitor.locator("text=You're chatting with Ada").waitFor();
    // And back: the visitor's next message arrives without reloading.
    await visitor.fill('textarea[aria-label="Message"]', 'Lovely, thanks');
    await visitor.press('textarea[aria-label="Message"]', 'Enter');
    await page.locator('[data-conversation] >> text=Lovely, thanks').waitFor({ timeout: 10000 });
  });
  await page.screenshot({ path: '45-chats.png' });
  await visitor.screenshot({ path: '46-chat-visitor.png' });

  await step('end it; the transcript is on the lead', async () => {
    await page.click('button:has-text("End chat")');
    await page.locator('text=ended the chat').waitFor();
    await visitor.locator('text=This chat has ended').waitFor({ timeout: 10000 });
    await page.click('[data-crm-link]');
    await page.locator('text=chatted on the website').waitFor();
    await page.click('[data-transcript] summary');
    await page.locator('[data-transcript] >> text=Yes, three left.').waitFor();
  });
} finally {
  await cleanupRun(s);
}
console.log(errors.length ? 'console errors:\n' + errors.join('\n') : 'no console errors');
await browser.close();
process.exit(failed || errors.length ? 1 : 0);
