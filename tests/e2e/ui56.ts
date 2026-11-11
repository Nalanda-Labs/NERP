// Live chat operations in the browser: a manager sets a chat button's automatic answer,
// consent text and a saved reply; a visitor must agree before chatting and gets the
// automatic answer; the operator answers with the saved reply (typing /code) and passes the
// chat to a colleague; the colleague ends it; the visitor rates it; the manager reviews it
// and sees it in the figures.
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
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `fail56-${n}-${i}.png`, fullPage: true }); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const ids: Record<string, string> = {};
for (const first of ['Kai', 'Lou']) ids[first] = (await admin.post('/users', { email: `${first.toLowerCase()}.${s}@acme.test`, password: 'password123', first_name: first, last_name: `Desk${s}` })).data.id;
const chan = (await admin.post('/chat/channels', { name: `Web ${s}`, operators: [ids.Kai, ids.Lou], distribution: 'all' })).data.items.find((c: any) => c.name === `Web ${s}`);

const newPage = async () => {
  const p = await (await browser.newContext({ viewport: { width: 1440, height: 1000 }, timezoneId: 'UTC', locale: 'en-GB' })).newPage();
  p.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
  p.on('console', (m) => m.type() === 'error' && !/401|400|404|409|Failed to load resource/.test(m.text()) && errors.push(m.text()));
  pages.push(p);
  return p;
};
const ctx = async (email: string, password = 'password123') => {
  const p = await newPage();
  await p.goto(B + '/login'); await p.fill('#email', email); await p.fill('#password', password);
  await p.click('button:has-text("Sign in")'); await p.waitForURL(B + '/');
  return p;
};

try {
  const ada = await ctx('admin@acme.test', 'supersecret1');
  const kai = await ctx(`kai.${s}@acme.test`);
  const lou = await ctx(`lou.${s}@acme.test`);
  const guest = await newPage();

  await step('the manager sets the automatic answer, consent and a saved reply', async () => {
    await ada.goto(B + '/chats/channels');
    await ada.click(`button[aria-label="Edit Web ${s}"]`);
    await ada.fill('#cf-auto', 'Thanks, we will be right with you.');
    await ada.fill('#cf-consent', 'I agree to the privacy notice.');
    await ada.click('button[form="ch-form"]');
    await ada.locator(`[data-channel="Web ${s}"]`).waitFor();
    // Wait for the dialog to close (on a slow machine its closing took the first field's text).
    await ada.locator('dialog[open]').waitFor({ state: 'detached' });
    await ada.fill('input[aria-label="Short code"]', `price${s}`);
    await ada.fill('input[aria-label="Title"]', 'Prices');
    await ada.fill('textarea[aria-label="Reply text"]', 'Our prices start at 10 euros a month.');
    await ada.click('[data-canned-admin] button:has-text("Add")');
    await ada.locator(`[data-canned-admin] >> text=/price${s}`).waitFor();
  });

  await step('a visitor agrees, chats, and gets the automatic answer', async () => {
    await guest.goto(`${B}/c/${chan.key}`);
    await guest.fill('#ch-name', `Mo ${s}`);
    await guest.click('button:has-text("Start chat")');
    // The consent box is required.
    if (await guest.locator('[data-messages]').count()) throw new Error('started without consent');
    await guest.check('input[type="checkbox"]');
    await guest.click('button:has-text("Start chat")');
    await guest.fill('textarea[aria-label="Message"]', 'How much is it?');
    await guest.click('button[aria-label="Send"]');
    await guest.locator('[data-msg="operator"]:has-text("Thanks, we will be right with you.")').waitFor({ timeout: 10000 });
  });

  await step('the operator answers with a saved reply and passes the chat on', async () => {
    await kai.goto(B + '/chats');
    await kai.click(`[data-chat="Mo ${s}"]`);
    await kai.fill('textarea[aria-label="Reply"]', `/price${s}`);
    await kai.click(`[data-canned] button:has-text("Prices")`);
    if ((await kai.inputValue('textarea[aria-label="Reply"]')) !== 'Our prices start at 10 euros a month.') throw new Error('saved reply not used');
    await kai.click('button:has-text("Send")');
    await guest.locator('[data-msg="operator"]:has-text("Our prices start at 10 euros")').waitFor({ timeout: 10000 });
    await kai.click('button:has-text("Pass on")');
    await kai.selectOption('#pass-u', { label: `Lou Desk${s}` });
    await kai.click('button[form="pass-form"]');
    await kai.locator('[data-conversation] >> text=/passed the chat to Lou/').waitFor();
  });

  await step('the colleague ends it and the visitor rates it', async () => {
    await lou.goto(B + '/chats');
    await lou.click('[role="tab"]:has-text("Mine")');
    await lou.click(`[data-chat="Mo ${s}"]`);
    lou.once('dialog', (d) => d.accept());
    await lou.click('button:has-text("End chat")');
    await guest.locator('text=How did we do?').waitFor({ timeout: 10000 });
    await guest.click('button[aria-label="5 out of 5"]');
    await guest.fill('textarea[aria-label="Comment"]', 'Lovely');
    await guest.click('form[aria-label="Rate the chat"] button:has-text("Send")');
    await guest.locator('[data-rated]').waitFor();
  });

  await step('the manager reviews it and sees the figures', async () => {
    await ada.goto(B + '/chats');
    await ada.click('[role="tab"]:has-text("Closed")');
    await ada.click(`[data-chat="Mo ${s}"]`);
    await ada.locator('[data-marks] >> text=★★★★★').waitFor();
    await ada.selectOption('select[aria-label="Review score"]', '4');
    await ada.fill('input[aria-label="Review comment"]', 'Good handover');
    await ada.click('[data-marks] button:has-text("Save")');
    await ada.locator('[data-marks] >> text=Review: 4/5').waitFor();
    await ada.click('a:has-text("Figures")');
    await ada.selectOption('select[aria-label="Chat button"]', { label: `Web ${s}` });
    await ada.locator('[data-totals] >> text=5.0 (1)').waitFor();
  });

  await step('no browser errors', async () => { if (errors.length) throw new Error(errors.join(' | ')); });
} finally {
  await browser.close();
  await cleanupRun(s);
}
console.log(failed ? `\n${failed} failed` : '\nall passed');
process.exit(failed ? 1 : 0);
