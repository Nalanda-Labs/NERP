// News feed in the browser: an important post with a poll; a colleague confirms reading,
// votes, likes and comments; the author sees who has read it.
import { chromium } from 'playwright';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
const B = 'http://localhost:8080';
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: string[] = [];
const s = Date.now().toString(36);
let n = 0, failed = 0;
const pages: import('playwright').Page[] = [];
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `fail30-${n}-${i}.png`, fullPage: true }); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
await admin.post('/users', { email: `kay.${s}@acme.test`, password: 'password123', first_name: 'Kay', last_name: `Reader${s}` });
const login = async (email: string, password: string) => {
  const p = await (await browser.newContext({ viewport: { width: 1280, height: 1000 } })).newPage();
  p.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
  p.on('console', (m) => m.type() === 'error' && !/401|400|404|Failed to load resource/.test(m.text()) && errors.push(m.text()));
  p.on('dialog', (d) => d.accept());
  pages.push(p);
  await p.goto(B + '/login'); await p.fill('#email', email); await p.fill('#password', password);
  await p.click('button:has-text("Sign in")'); await p.waitForURL(B + '/');
  return p;
};
const ada = await login('admin@acme.test', 'supersecret1');
const kay = await login(`kay.${s}@acme.test`, 'password123');
const text = `New parking rules ${s}`;

try {
  await step('an important post with a poll', async () => {
    await ada.click('aside a:has-text("News feed")');
    await ada.fill('textarea[aria-label="Post"]', text);
    await ada.click('button:has-text("Poll")');
    await ada.fill('input[aria-label="Poll question"]', 'Happy with them?');
    await ada.fill('input[aria-label="Answer 1"]', 'Yes');
    await ada.fill('input[aria-label="Answer 2"]', 'No');
    await ada.check('label:has-text("Important") input');
    await ada.click('form[aria-label="New post"] button:has-text("Post")');
    await ada.locator(`[data-post^="${text}"]`).waitFor();
  });

  await step('a colleague reads, votes, likes, comments', async () => {
    await kay.click('aside a:has-text("News feed")');
    const post = kay.locator(`section[aria-label="Important"] [data-post^="${text}"]`);
    await post.locator('button:has-text("I\'ve read it")').click();
    await post.locator("text=You've read this").waitFor();
    await post.locator('[data-poll] button:has-text("Yes")').click();
    await post.locator('[data-poll] >> text=1 vote').waitFor();
    await post.locator('[data-like]').click();
    await post.locator('[data-like][aria-pressed="true"]').waitFor();
    await post.locator('button:has-text("Comment")').click();
    await post.locator('input[aria-label="Write a comment"]').fill('Makes sense');
    await post.locator('button:has-text("Send")').click();
    await post.locator('[data-comment="Makes sense"]').waitFor();
  });

  await step('the author sees who has read it', async () => {
    await ada.reload();
    const post = ada.locator(`[data-post^="${text}"]`);
    await post.locator('button:has-text("Read by 1")').click();
    await post.locator(`[data-readers] >> text=Kay Reader${s}`).waitFor();
  });
  await kay.screenshot({ path: '50-feed.png' });
} finally {
  await cleanupRun(s);
}
console.log(errors.length ? 'console errors:\n' + errors.join('\n') : 'no console errors');
await browser.close();
process.exit(failed || errors.length ? 1 : 0);
