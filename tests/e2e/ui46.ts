// Call host controls in the browser: the host mutes everyone and turns the others'
// microphones off (their button is then off and can't be turned on); then both see the call
// in their history.
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
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `fail46-${n}-${i}.png`, fullPage: true }); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const bobUser = (await admin.post('/users', { email: `bob.${s}@acme.test`, password: 'password123', first_name: 'Bob', last_name: `Host${s}` })).data;
const chat = (await admin.post('/im/chats', { title: `Huddle ${s}`, members: [bobUser.id] })).data.id;

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
  await step('both in the call', async () => {
    await ada.goto(`${B}/messenger?chat=${chat}`);
    await ada.click('[data-call-button]');
    await ada.waitForURL(/\/call\//);
    await ada.locator('[data-call-status="in"]').waitFor();
    await bob.goto(ada.url());
    await bob.locator('[data-call-status="in"]').waitFor();
    await ada.locator('[data-remote]').waitFor();
  });

  await step('only the host has host controls', async () => {
    await ada.locator('[data-host-controls]').waitFor();
    if (await bob.locator('[data-host-controls]').count()) throw new Error('a guest sees host controls');
  });

  await step('mute everyone', async () => {
    await ada.click('[data-host-controls]');
    await ada.click('button:has-text("Mute everyone")');
    await bob.locator('button[aria-label="Unmute"]').waitFor();
    await bob.locator('text=The host muted everyone').waitFor();
    // Allowed: Bob can turn it back on.
    await bob.click('button[aria-label="Unmute"]');
    await bob.locator('button[aria-label="Mute"]').waitFor();
  });

  await step('microphones off for the others', async () => {
    await ada.uncheck('label:has-text("Others may use their microphone") input');
    await bob.locator('button[aria-label="Unmute"][disabled]').waitFor();
    await ada.check('label:has-text("Others may use their microphone") input');
    await bob.locator('button[aria-label="Unmute"]:not([disabled])').waitFor();
  });

  await step('call history', async () => {
    await bob.click('button:has-text("Leave")');
    await ada.click('button:has-text("Leave")');
    await bob.goto(B + '/calls');
    await bob.locator(`li:has-text("Huddle ${s}")`).first().waitFor();
    await bob.locator(`li:has-text("Huddle ${s}"):has-text("called")`).first().waitFor();
  });
} finally {
  await browser.close();
  await cleanupRun(s);
}
console.log(errors.length ? 'console errors:\n' + errors.join('\n') : 'no console errors');
process.exit(failed || errors.length ? 1 : 0);
