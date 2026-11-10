// Calls through LiveKit in the browser: two people (fake cameras) in a call through the media
// server see and hear each other; recording from the call (the badge both see, stopping it,
// the recording posted in the chat); a clear message when the recording service is down.
import { chromium } from 'playwright';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { LIVEKIT_ENV, LIVEKIT_SERVER, clearRecordings, startFakeEgress, startLiveKit } from './livekit-server.ts';
const B = 'http://localhost:8080';
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const s = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const mk = async (first: string) => (await admin.post('/users', { email: `${first.toLowerCase()}.${s}@acme.test`, password: 'password123', first_name: first, last_name: `Live${s}` })).data;
const ann = await mk('Ann');
await mk('Bob');
if (!LIVEKIT_SERVER) { await cleanupRun(s); console.log('skipped: livekit-server is not installed'); process.exit(0); }
const stopServer = await startLiveKit();
const egress = await startFakeEgress();
await new Promise((r) => setTimeout(r, 5500)); // NERP re-checks the server at most every 5 seconds
{
  const probe = new Client();
  await probe.post('/auth/login', { email: `ann.${s}@acme.test`, password: 'password123' });
  if ((await probe.get('/calls/00000000-0000-0000-0000-000000000000')).status !== 404 || !(await fetch(`${B}/api/setup`)).ok) throw new Error('server not ready');
}

const browser = await chromium.launch({ executablePath: exe, args: ['--use-fake-device-for-media-stream', '--use-fake-ui-for-media-stream'] });
const errors: string[] = [];
let n = 0, failed = 0;
const pages: import('playwright').Page[] = [];
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `fail41-${n}-${i}.png`, fullPage: true }); }
};
const login = async (first: string) => {
  const ctx = await browser.newContext({ viewport: { width: 1200, height: 900 }, permissions: ['camera', 'microphone'] });
  const p = await ctx.newPage();
  p.on('pageerror', (e) => errors.push(`${first}: PAGEERROR ${e.message}`));
  p.on('console', (m) => m.type() === 'error' && !/401|400|404|409|Failed to load resource/.test(m.text()) && errors.push(`${first}: ${m.text()}`));
  pages.push(p);
  await p.goto(B + '/login'); await p.fill('#email', `${first.toLowerCase()}.${s}@acme.test`); await p.fill('#password', 'password123');
  await p.click('button:has-text("Sign in")'); await p.waitForURL(B + '/');
  return p;
};
const bob = await login('Bob');
const annP = await login('Ann');
let skipped = false;

try {
  await step('a call through the media server', async () => {
    await bob.goto(`${B}/employees/${ann.id}`);
    await bob.click('button:has-text("Message")');
    await bob.waitForURL(/\/messenger\?chat=/);
    await bob.click('[data-call-button]');
    await bob.waitForURL(/\/call\//);
    await bob.locator('[data-call-status="in"]').waitFor();
    const media = await bob.locator('[data-call-status]').getAttribute('data-call-media');
    if (media !== 'livekit') { skipped = true; return; }
    await annP.locator('[data-ringing]').waitFor({ timeout: 10000 });
    await annP.click('[data-ringing] button:has-text("Join")');
    await annP.locator(`[data-remote="Bob Live${s}"][data-peer-state="connected"]`).waitFor({ timeout: 20000 });
    await bob.locator(`[data-remote="Ann Live${s}"][data-peer-state="connected"]`).waitFor({ timeout: 20000 });
  });
  if (skipped) throw new Error('SKIP');

  await step('they see each other', async () => {
    for (const [p, who] of [[annP, 'Bob'], [bob, 'Ann']] as const) {
      const playing = await p.locator(`[data-remote="${who} Live${s}"] video`).evaluate(async (v: HTMLVideoElement) => {
        for (let i = 0; i < 80 && v.videoWidth === 0; i++) await new Promise((r) => setTimeout(r, 100));
        return v.videoWidth > 0;
      });
      if (!playing) throw new Error(`no video from ${who}`);
    }
  });

  await step('the recording service down: said plainly', async () => {
    egress.unavailable = true;
    await annP.click('button:has-text("Record")');
    await annP.locator('text=The recording service (LiveKit Egress) is not running').waitFor();
  });

  await step('recording: both see it', async () => {
    await annP.click('button:has-text("Record")');
    await annP.locator(`[data-recording] >> text=started by Ann Live${s}`).waitFor();
    await bob.locator(`[data-recording] >> text=started by Ann Live${s}`).waitFor();
  });
  await bob.screenshot({ path: '62-call-recording.png' });

  await step('stopping it', async () => {
    await bob.click('button:has-text("Stop recording")');
    await bob.locator('[data-recording]').waitFor({ state: 'detached' });
    await annP.locator('[data-recording]').waitFor({ state: 'detached' });
  });

  await step('leaving; the recording appears in the chat', async () => {
    await annP.click('button:has-text("Leave")');
    await annP.waitForURL(/\/messenger\?chat=/);
    await bob.click('button:has-text("Leave")');
    await bob.waitForURL(/\/messenger\?chat=/);
    const convo = bob.getByRole('region', { name: 'Conversation' });
    await convo.getByText(/^Recording of the call \(1 min \d+ s\)$/).waitFor({ timeout: 45000 });
    await convo.getByText(/Call recording .*\.mp4/).waitFor();
  });
} catch (e: any) {
  if (e.message !== 'SKIP') throw e;
} finally {
  await browser.close();
  stopServer();
  await egress.close();
  clearRecordings();
  await cleanupRun(s);
}
if (skipped) {
  console.log(`skipped: start the server with ${Object.entries(LIVEKIT_ENV).map(([k, v]) => `${k}=${v}`).join(' ')}`);
  process.exit(0);
}
console.log(errors.length ? 'console errors:\n' + errors.join('\n') : 'no console errors');
process.exit(failed || errors.length ? 1 : 0);
