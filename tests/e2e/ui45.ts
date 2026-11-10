// The messenger in the browser: a channel with posts and comments, finding and joining an
// open chat, a poll, a reaction, a sticker, a voice message recorded with a (fake)
// microphone, a task from a message, the general chat, muting and a folder.
import { chromium } from 'playwright';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
const B = 'http://localhost:8080';
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe, args: ['--use-fake-device-for-media-stream', '--use-fake-ui-for-media-stream'] });
const errors: string[] = [];
const s = Date.now().toString(36);
let n = 0, failed = 0;
const pages: import('playwright').Page[] = [];
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `fail45-${n}-${i}.png`, fullPage: true }); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
await admin.post('/users', { email: `bea.${s}@acme.test`, password: 'password123', first_name: 'Bea', last_name: `Chat${s}` });

const login = async (email: string, password: string) => {
  const p = await (await browser.newContext({ viewport: { width: 1440, height: 900 }, permissions: ['camera', 'microphone'] })).newPage();
  p.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
  p.on('console', (m) => m.type() === 'error' && !/401|400|404|Failed to load resource/.test(m.text()) && errors.push(m.text()));
  pages.push(p);
  await p.goto(B + '/login'); await p.fill('#email', email); await p.fill('#password', password);
  await p.click('button:has-text("Sign in")'); await p.waitForURL(B + '/');
  return p;
};
const ada = await login('admin@acme.test', 'supersecret1');
const bea = await login(`bea.${s}@acme.test`, 'password123');
const dialog = (p: import('playwright').Page) => p.locator('dialog[open]');

try {
  await step('make an open channel', async () => {
    await ada.goto(B + '/messenger');
    await ada.click('button:has-text("New")');
    await dialog(ada).locator('button:has-text("Channel")').click();
    await ada.fill('#grp-t', `Launches ${s}`);
    await ada.fill('#grp-d', 'Release notes');
    await ada.selectOption('#grp-v', 'open');
    await dialog(ada).locator('button:has-text("Create")').click();
    await ada.locator(`h2:has-text("Launches ${s}")`).waitFor();
    await ada.fill('textarea[aria-label="Message"]', `Version 2 is out ${s}`);
    await ada.press('textarea[aria-label="Message"]', 'Enter');
    await ada.locator(`[data-msg="Version 2 is out ${s}"]`).waitFor();
  });

  await step('someone finds it, reads it, joins and comments', async () => {
    await bea.goto(B + '/messenger');
    await bea.click('button[aria-label="Find open chats"]');
    await bea.fill('input[aria-label="Search open chats"]', `Launches ${s}`);
    await bea.click(`[data-found="Launches ${s}"]`);
    await bea.locator(`[data-msg="Version 2 is out ${s}"]`).waitFor();
    await bea.locator('[data-preview]').waitFor();
    await bea.click('[data-preview] button:has-text("Join")');
    await bea.locator('[data-read-only]:has-text("You can comment")').waitFor();
    await bea.click(`[data-msg="Version 2 is out ${s}"] [data-comments]`);
    await bea.fill('aside[aria-label="Comments"] textarea', `Great news ${s}`);
    await bea.press('aside[aria-label="Comments"] textarea', 'Enter');
    await bea.locator(`aside [data-msg="Great news ${s}"]`).waitFor();
    // The poster sees the count go up live.
    await ada.locator(`[data-msg="Version 2 is out ${s}"] [data-comments]:has-text("1 comment")`).waitFor();
  });

  await step('a poll, voted live', async () => {
    await ada.click('button[aria-label="Poll"]');
    await ada.fill('#poll-q', `Ship on Friday ${s}?`);
    await ada.fill('input[aria-label="Answer 1"]', 'Yes');
    await ada.fill('input[aria-label="Answer 2"]', 'No');
    await ada.click('button:has-text("Post the poll")');
    await bea.locator(`[data-poll="Ship on Friday ${s}?"]`).waitFor();
    await bea.click(`[data-poll="Ship on Friday ${s}?"] [data-option="Yes"]`);
    await bea.locator(`[data-poll="Ship on Friday ${s}?"] [data-option="Yes"]:has-text("✓")`).waitFor();
    await ada.locator(`[data-poll="Ship on Friday ${s}?"]:has-text("1 vote")`).waitFor();
  });

  await step('a reaction and a sticker', async () => {
    await bea.click('aside button[aria-label="Close comments"]');
    const msg = bea.locator(`[data-messages] [data-msg="Version 2 is out ${s}"]`);
    await msg.hover();
    await msg.locator('button[aria-label="React"]').click();
    await bea.locator('[role=menu][aria-label="Reactions"] button:has-text("🎉")').click();
    await ada.locator(`[data-messages] [data-msg="Version 2 is out ${s}"] [data-reaction="🎉"]:has-text("1")`).waitFor();
    await ada.click('button[aria-label="Sticker"]');
    await ada.click('[role=menu][aria-label="Stickers"] button[aria-label="Rocket"]');
    await bea.locator('[data-sticker="rocket"]').waitFor();
  });

  await step('a group chat: a voice message from the microphone', async () => {
    await ada.click('button:has-text("New")');
    await dialog(ada).locator('button:has-text("Group chat")').click();
    await ada.fill('#grp-t', `Team ${s}`);
    await dialog(ada).locator('input[placeholder*="Search"], input[type=search], input').last().fill('Bea');
    await ada.locator(`text=Bea Chat${s}`).first().click();
    await dialog(ada).locator('button:has-text("Create")').click();
    await ada.locator(`h2:has-text("Team ${s}")`).waitFor();
    await ada.click('button[aria-label="Record a voice message"]');
    await ada.locator('[data-recording="voice"]').waitFor();
    await ada.waitForTimeout(1500);
    await ada.click('[data-recording] button:has-text("Send")');
    await ada.locator('[data-kind="voice"] audio[data-voice]').waitFor();
    await bea.click(`[data-chat="Team ${s}"]`);
    await bea.locator('[data-kind="voice"] audio[data-voice]').waitFor();
  });

  await step('a task from a message', async () => {
    await bea.fill('textarea[aria-label="Message"]', `Please update the docs ${s}`);
    await bea.press('textarea[aria-label="Message"]', 'Enter');
    const msg = ada.locator(`[data-msg="Please update the docs ${s}"]`);
    await msg.hover();
    await msg.locator('button[aria-label="Make a task"]').click();
    await ada.locator('#mk-title').waitFor();
    if ((await ada.inputValue('#mk-title')) !== `Please update the docs ${s}`) throw new Error('title not taken from the message');
    await ada.click('button:has-text("Make the task")');
    await ada.locator('[data-system]:has-text("made a task")').waitFor();
  });

  await step('mute and a folder', async () => {
    await bea.click('[data-chat-info]');
    await bea.click('[data-mute]');
    await bea.locator('[data-mute]:has-text("Unmute")').waitFor();
    await bea.keyboard.press('Escape');
    await bea.locator(`[data-chat="Team ${s}"]:has-text("🔕")`).waitFor();
    await bea.click('button[aria-label="New folder"]');
    await bea.fill('#fd-name', `Work ${s}`);
    await bea.click('button:has-text("Save")');
    await bea.click(`[data-folder="Work ${s}"]`);
    await bea.locator(`[data-chat="Team ${s}"]`).waitFor();
    await bea.locator(`[data-chat="Launches ${s}"]`).waitFor({ state: 'detached' });
  });

  await step('the general chat', async () => {
    await bea.click('[data-general]');
    await bea.locator('h2:has-text("General")').waitFor();
    await bea.locator('textarea[aria-label="Message"]').waitFor();
  });
} finally {
  await browser.close();
  await cleanupRun(s);
}
console.log(errors.length ? 'console errors:\n' + errors.join('\n') : 'no console errors');
process.exit(failed || errors.length ? 1 : 0);
