// Guests and background blur in the browser: a chat's owner invites a guest from the chat's
// settings; the guest joins from the email, sees only their menu (messenger, workgroups,
// tasks, boards) and their chat; in a call the owner blurs their background (a fake camera).
import { chromium } from 'playwright';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
const B = 'http://localhost:8080';
const MP = 'http://localhost:8026/api/v1';
const s = Date.now().toString(36);
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe, args: ['--use-fake-device-for-media-stream', '--use-fake-ui-for-media-stream'] });
const errors: string[] = [];
let n = 0, failed = 0;
const pages: import('playwright').Page[] = [];
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `fail65-${n}-${i}.png`, fullPage: true }); }
};
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const mailLink = async (to: string) => {
  for (let i = 0; i < 40; i++) {
    const r = await (await fetch(`${MP}/search?query=${encodeURIComponent('to:' + to)}&limit=5`)).json();
    const m = r.messages?.find((x: any) => x.Subject.includes('invited you'));
    if (m) { const full = await (await fetch(`${MP}/message/${m.ID}`)).json(); return `${B}/accept-invite${full.Text.match(/accept-invite(\?token=[\w-]+)/)[1]}`; }
    await sleep(250);
  }
  throw new Error('no mail');
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const ownerEmail = `olga.${s}@acme.test`;
await admin.post('/users', { email: ownerEmail, password: 'password123', first_name: 'Olga', last_name: `Own${s}` });
const olgaApi = new Client();
await olgaApi.post('/auth/login', { email: ownerEmail, password: 'password123' });
const helper = (await admin.post('/users', { email: `hal.${s}@acme.test`, password: 'password123', first_name: 'Hal', last_name: `Help${s}` })).data;
const chat = (await olgaApi.post('/im/chats', { title: `Supplier ${s}`, members: [helper.id] })).data;
const guestEmail = `vera.${s}@acme.test`;

const ctx = async (email?: string) => {
  const p = await (await browser.newContext({ viewport: { width: 1280, height: 900 }, timezoneId: 'UTC', locale: 'en-GB', permissions: ['camera', 'microphone'] })).newPage();
  p.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
  p.on('console', (m) => m.type() === 'error' && !/401|400|403|404|409|Failed to load resource|WebGL|GPU|gpu|delegate/.test(m.text()) && errors.push(m.text()));
  pages.push(p);
  if (email) {
    await p.goto(B + '/login'); await p.fill('#email', email); await p.fill('#password', 'password123');
    await p.click('button:has-text("Sign in")'); await p.waitForURL(B + '/');
  }
  return p;
};

try {
  const olga = await ctx(ownerEmail);

  await step('invite a guest from the chat', async () => {
    await olga.goto(`${B}/messenger?chat=${chat.id}`);
    await olga.click('[data-chat-info]');
    await olga.click('button:has-text("Invite a guest from outside the company")');
    await olga.fill('dialog input[aria-label="Guest\'s email"]', guestEmail);
    await olga.fill('dialog input[aria-label="Guest\'s first name"]', 'Vera');
    await olga.fill('dialog input[aria-label="Guest\'s last name"]', `Out${s}`);
    await olga.click('dialog [data-guest-invite] button:has-text("Invite")');
    await olga.locator(`text=${guestEmail} is invited as a guest`).waitFor();
    await olga.locator(`dialog [data-person="Vera Out${s}"]:has-text("Guest")`).waitFor();
  });

  await step('the guest joins and sees only their part', async () => {
    const vera = await ctx();
    await vera.goto(await mailLink(guestEmail));
    await vera.fill('#pw', 'verapassword1');
    await vera.fill('#pw2', 'verapassword1');
    await vera.click('button:has-text("Join and sign in")');
    await vera.waitForURL(B + '/profile');
    await vera.locator('[data-guest-badge]').waitFor();
    const menu = (await vera.locator('aside nav a').allTextContents()).map((t) => t.replace(/\d+/g, '').trim());
    if (JSON.stringify(menu) !== JSON.stringify(['Messenger', 'Workgroups', 'Tasks', 'Boards'])) throw new Error(`menu: ${menu.join(', ')}`);
    await vera.goto(B + '/');
    await vera.waitForURL(/\/messenger/);
    await vera.locator(`text=Supplier ${s}`).first().waitFor();
    await vera.screenshot({ path: '/tmp/nerp-e2e/guest-view.png' });
  });

  await step('blur the background in a call', async () => {
    const call = (await olgaApi.post('/calls', { chat_id: chat.id })).data;
    await olga.goto(`${B}/call/${call.id}`);
    await olga.locator('[data-blur]:not([disabled])').waitFor({ timeout: 30000 });
    await olga.click('[data-blur]');
    await olga.locator('[data-blur][aria-pressed="true"]').waitFor({ timeout: 60000 });
    await sleep(1500);
    await olga.screenshot({ path: '/tmp/nerp-e2e/call-blur.png' });
    await olga.click('[data-blur]');
    await olga.locator('[data-blur][aria-pressed="false"]').waitFor();
  });

  await step('no browser errors', async () => {
    if (errors.length) throw new Error(errors.join(' | '));
  });
} finally {
  await browser.close();
  await cleanupRun(s);
}
console.log(failed ? `\n${failed} failed` : '\nall passed');
process.exit(failed ? 1 : 0);
