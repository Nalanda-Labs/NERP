// Video calls in the browser: two people with (fake) cameras; one calls from Messenger, the
// other's browser rings, joins, and the two connect directly; then they leave.
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
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `fail34-${n}-${i}.png`, fullPage: true }); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const mk = async (first: string) => (await admin.post('/users', { email: `${first.toLowerCase()}.${s}@acme.test`, password: 'password123', first_name: first, last_name: `Video${s}` })).data;
const ann = await mk('Ann');
await mk('Bob');
const login = async (first: string) => {
  const ctx = await browser.newContext({ viewport: { width: 1200, height: 900 }, permissions: ['camera', 'microphone'] });
  const p = await ctx.newPage();
  p.on('pageerror', (e) => errors.push(`${first}: PAGEERROR ${e.message}`));
  p.on('console', (m) => m.type() === 'error' && !/401|400|404|Failed to load resource/.test(m.text()) && errors.push(`${first}: ${m.text()}`));
  pages.push(p);
  await p.goto(B + '/login'); await p.fill('#email', `${first.toLowerCase()}.${s}@acme.test`); await p.fill('#password', 'password123');
  await p.click('button:has-text("Sign in")'); await p.waitForURL(B + '/');
  return p;
};
const bob = await login('Bob');
const annP = await login('Ann');

try {
  await step('call from Messenger', async () => {
    await bob.goto(`${B}/employees/${ann.id}`);
    await bob.click('button:has-text("Message")');
    await bob.waitForURL(/\/messenger\?chat=/);
    await bob.click('[data-call-button]');
    await bob.waitForURL(/\/call\//);
    await bob.locator('[data-call-status="in"]').waitFor();
    await bob.locator(`text=Waiting for Ann Video${s}`).waitFor();
  });

  await step('it rings; she joins; they connect', async () => {
    await annP.locator('[data-ringing]').waitFor({ timeout: 10000 });
    await annP.click('[data-ringing] button:has-text("Join")');
    await annP.locator(`[data-remote="Bob Video${s}"][data-peer-state="connected"]`).waitFor({ timeout: 20000 });
    await bob.locator(`[data-remote="Ann Video${s}"][data-peer-state="connected"]`).waitFor({ timeout: 20000 });
    // Bob's picture reaches Ann.
    const playing = await annP.locator(`[data-remote="Bob Video${s}"] video`).evaluate(async (v: HTMLVideoElement) => {
      for (let i = 0; i < 50 && v.videoWidth === 0; i++) await new Promise((r) => setTimeout(r, 100));
      return v.videoWidth > 0;
    });
    if (!playing) throw new Error('no video from Bob');
  });
  await annP.screenshot({ path: '54-call.png' });

  await step('mute, then leave; the call ends', async () => {
    await annP.click('button[aria-label="Mute"]');
    await annP.locator('text=You · muted').waitFor();
    await annP.click('button:has-text("Leave")');
    await annP.waitForURL(/\/messenger\?chat=/);
    await bob.locator(`[data-remote="Ann Video${s}"]`).waitFor({ state: 'detached', timeout: 10000 });
    await bob.click('button:has-text("Leave")');
    await bob.waitForURL(/\/messenger\?chat=/);
    await bob.locator('[data-system]:text-matches("Call ended · ")').waitFor({ timeout: 10000 });
  });
} finally {
  await cleanupRun(s);
}
console.log(errors.length ? 'console errors:\n' + errors.join('\n') : 'no console errors');
await browser.close();
process.exit(failed || errors.length ? 1 : 0);
