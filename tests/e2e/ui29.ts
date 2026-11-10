// Messenger in the browser: message a colleague from their profile, they see it live with
// an unread badge, reply, and the first person sees the reply and "Seen". Then from the
// Messenger itself: a new chat with one person, and a video call with one person.
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
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `fail29-${n}-${i}.png`, fullPage: true }); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const bobUser = (await admin.post('/users', { email: `bob.${s}@acme.test`, password: 'password123', first_name: 'Bob', last_name: `Chat${s}` })).data;
await admin.post('/users', { email: `cy.${s}@acme.test`, password: 'password123', first_name: 'Cy', last_name: `Chat${s}` });
await admin.post('/users', { email: `dee.${s}@acme.test`, password: 'password123', first_name: 'Dee', last_name: `Chat${s}` });
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
const bob = await login(`bob.${s}@acme.test`, 'password123');

try {
  await step('message a colleague from their profile', async () => {
    await ada.goto(`${B}/employees/${bobUser.id}`);
    await ada.click('button:has-text("Message")');
    await ada.waitForURL(/\/messenger\?chat=/);
    await ada.fill('textarea[aria-label="Message"]', `Hello Bob ${s}`);
    await ada.press('textarea[aria-label="Message"]', 'Enter');
    await ada.locator(`[data-msg="Hello Bob ${s}"]`).waitFor();
  });

  await step('it arrives live, with an unread badge', async () => {
    await bob.locator('aside a:has-text("Messenger") >> text=1').waitFor({ timeout: 10000 });
    await bob.click('aside a:has-text("Messenger")');
    await bob.locator('[data-chat="Ada Admin"] [data-unread]').waitFor();
    await bob.click('[data-chat="Ada Admin"]');
    await bob.locator(`[data-msg="Hello Bob ${s}"]`).waitFor();
    await bob.locator('aside a:has-text("Messenger") >> text=1').waitFor({ state: 'detached', timeout: 10000 });
  });

  await step('reply; seen', async () => {
    await ada.locator('[data-seen]').waitFor({ timeout: 10000 });
    await bob.fill('textarea[aria-label="Message"]', 'Hi Ada!');
    await bob.press('textarea[aria-label="Message"]', 'Enter');
    await ada.locator('[data-msg="Hi Ada!"]').waitFor({ timeout: 10000 });
    // Read at once, being open.
    await ada.locator('aside a:has-text("Messenger") >> text=1').waitFor({ state: 'detached', timeout: 10000 });
  });
  await ada.screenshot({ path: '49-messenger.png' });

  // Three ways to choose someone: click them in the list, type and press Enter, paste the name.
  const choose = async (name: string, how: 'click' | 'enter' | 'paste') => {
    await ada.click('aside a:has-text("Messenger")');
    await ada.click('button:has-text("New")');
    const input = ada.locator('dialog input[placeholder="Type a name…"]');
    if (how === 'paste') {
      await input.focus();
      await input.evaluate((el: HTMLInputElement, v) => { el.value = v; el.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'insertFromPaste', data: v })); }, name);
    } else if (how === 'enter') {
      await input.pressSequentially(name, { delay: 5 });
      await ada.keyboard.press('Enter');
    } else {
      await input.fill(name);
      await ada.click(`dialog button:has-text("${name}")`);
    }
    await ada.locator(`[data-direct-with]:has-text("${name}")`).waitFor();
  };
  await step('choosing by clicking, then changing your mind', async () => {
    await choose(`Bob Chat${s}`, 'click');
    await ada.click('[data-direct-with] button:has-text("Change")');
    await ada.locator('dialog input[placeholder="Type a name…"]').waitFor();
    await ada.keyboard.press('Escape');
  });
  await step('a new chat with one person, from the Messenger', async () => {
    await choose(`Cy Chat${s}`, 'enter');
    await ada.keyboard.press('Enter'); // "Start chat" has the focus
    await ada.waitForURL(/\/messenger\?chat=/);
    await ada.locator(`[data-chat="Cy Chat${s}"]`).waitFor();
    await ada.fill('textarea[aria-label="Message"]', `Hi Cy ${s}`);
    await ada.press('textarea[aria-label="Message"]', 'Enter');
    await ada.locator(`[data-msg="Hi Cy ${s}"]`).waitFor();
  });

  await step('a video call with one person, from the Messenger', async () => {
    await choose(`Dee Chat${s}`, 'paste');
    await ada.click('dialog button:has-text("Video call")');
    await ada.waitForURL(/\/call\//);
    await ada.locator('[data-call-status="in"]').waitFor();
    await ada.locator(`text=Waiting for Dee Chat${s}`).waitFor();
    await ada.click('button:has-text("Leave")');
    await ada.waitForURL(/\/messenger\?chat=/);
  });
} finally {
  await cleanupRun(s);
}
console.log(errors.length ? 'console errors:\n' + errors.join('\n') : 'no console errors');
await browser.close();
process.exit(failed || errors.length ? 1 : 0);
