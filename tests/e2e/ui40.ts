// Video calls through a TURN server (coturn): both browsers may only use relayed connections
// (as when they are on networks that can't reach each other), so the call works only if
// NERP hands out the TURN server and the media really goes through it. Checked with the
// browsers' own connection statistics and the TURN server's log.
import { chromium } from 'playwright';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { TURNSERVER, TURN_ICE_SERVERS, startTurn } from './turn-server.ts';
const B = 'http://localhost:8080';
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const s = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const mk = async (first: string) => (await admin.post('/users', { email: `${first.toLowerCase()}.${s}@acme.test`, password: 'password123', first_name: first, last_name: `Relay${s}` })).data;
const ann = await mk('Ann');
const bobUser = await mk('Bob');

// Does this server hand out the test TURN server?
const probe = new Client();
await probe.post('/auth/login', { email: `ann.${s}@acme.test`, password: 'password123' });
const call = (await probe.post('/calls', { users: [bobUser.id] })).data;
const offered = (await probe.post(`/calls/${call.id}/join`)).data?.ice_servers ?? [];
await probe.post(`/calls/${call.id}/leave`);
const skip = !TURNSERVER ? 'coturn (turnserver) is not installed'
  : !offered.some((o: any) => o.urls === TURN_ICE_SERVERS[0].urls && o.username === TURN_ICE_SERVERS[0].username && o.credential === TURN_ICE_SERVERS[0].credential)
    ? `start the server with WEBRTC_ICE_SERVERS='${JSON.stringify(TURN_ICE_SERVERS)}'`
  : null;
if (skip) {
  await cleanupRun(s);
  console.log(`skipped: ${skip}`);
  process.exit(0);
}

const turn = await startTurn(`nerp-e2e-turn-${s}`);
const browser = await chromium.launch({ executablePath: exe, args: ['--use-fake-device-for-media-stream', '--use-fake-ui-for-media-stream'] });
const errors: string[] = [];
let n = 0, failed = 0;
const pages: import('playwright').Page[] = [];
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `fail40-${n}-${i}.png`, fullPage: true }); }
};
const login = async (first: string) => {
  const ctx = await browser.newContext({ viewport: { width: 1200, height: 900 }, permissions: ['camera', 'microphone'] });
  // Relayed connections only, and keep the connections where the test can read their statistics.
  await ctx.addInitScript(() => {
    const Original = window.RTCPeerConnection;
    const all: RTCPeerConnection[] = [];
    (window as any).__pcs = all;
    const Relayed = function (config?: RTCConfiguration) {
      const pc = new Original({ ...(config ?? {}), iceTransportPolicy: 'relay' });
      all.push(pc);
      return pc;
    } as unknown as typeof RTCPeerConnection;
    Relayed.prototype = Original.prototype;
    (window as any).RTCPeerConnection = Relayed;
  });
  const p = await ctx.newPage();
  p.on('pageerror', (e) => errors.push(`${first}: PAGEERROR ${e.message}`));
  p.on('console', (m) => m.type() === 'error' && !/401|400|404|Failed to load resource/.test(m.text()) && errors.push(`${first}: ${m.text()}`));
  pages.push(p);
  await p.goto(B + '/login'); await p.fill('#email', `${first.toLowerCase()}.${s}@acme.test`); await p.fill('#password', 'password123');
  await p.click('button:has-text("Sign in")'); await p.waitForURL(B + '/');
  return p;
};
/** The connection actually in use: its local and remote candidate types and bytes received. */
const route = (p: import('playwright').Page) => p.evaluate(async () => {
  const out: { local: string; remote: string; received: number; video: number }[] = [];
  for (const pc of (window as any).__pcs as RTCPeerConnection[]) {
    if (pc.connectionState !== 'connected') continue;
    const stats = await pc.getStats();
    let pair: any, video = 0;
    stats.forEach((r: any) => {
      if (r.type === 'transport' && r.selectedCandidatePairId) pair = stats.get(r.selectedCandidatePairId);
      if (r.type === 'inbound-rtp' && r.kind === 'video') video += r.framesDecoded ?? 0;
    });
    if (!pair) stats.forEach((r: any) => { if (r.type === 'candidate-pair' && r.nominated && r.state === 'succeeded') pair = r; });
    if (pair) out.push({ local: stats.get(pair.localCandidateId)?.candidateType, remote: stats.get(pair.remoteCandidateId)?.candidateType, received: pair.bytesReceived, video });
  }
  return out;
});

const bob = await login('Bob');
const annP = await login('Ann');

try {
  await step('a call where only relayed connections are allowed', async () => {
    await bob.goto(`${B}/employees/${ann.id}`);
    await bob.click('button:has-text("Message")');
    await bob.waitForURL(/\/messenger\?chat=/);
    await bob.click('[data-call-button]');
    await bob.waitForURL(/\/call\//);
    await annP.locator('[data-ringing]').waitFor({ timeout: 10000 });
    await annP.click('[data-ringing] button:has-text("Join")');
    await annP.locator(`[data-remote="Bob Relay${s}"][data-peer-state="connected"]`).waitFor({ timeout: 30000 });
    await bob.locator(`[data-remote="Ann Relay${s}"][data-peer-state="connected"]`).waitFor({ timeout: 30000 });
  });

  await step('the picture arrives', async () => {
    const playing = await annP.locator(`[data-remote="Bob Relay${s}"] video`).evaluate(async (v: HTMLVideoElement) => {
      for (let i = 0; i < 80 && v.videoWidth === 0; i++) await new Promise((r) => setTimeout(r, 100));
      return v.videoWidth > 0;
    });
    if (!playing) throw new Error('no video from Bob');
  });
  await annP.screenshot({ path: '61-call-relayed.png' });

  await step('through the TURN server, both ways', async () => {
    await annP.waitForTimeout(1500);
    for (const [who, p] of [['Ann', annP], ['Bob', bob]] as const) {
      const r = await route(p);
      if (!r.length) throw new Error(`${who}: no connection in use`);
      for (const c of r) {
        if (c.local !== 'relay' || c.remote !== 'relay') throw new Error(`${who}: not relayed: ${JSON.stringify(r)}`);
        if (!(c.received > 10_000) || !(c.video > 10)) throw new Error(`${who}: little or no media: ${JSON.stringify(r)}`);
      }
    }
  });

  await step('the TURN server saw the allocations', async () => {
    const log = turn.log();
    const allocations = (log.match(/ALLOCATE processed, success/g) ?? []).length;
    if (allocations < 2) throw new Error(`only ${allocations} allocations in the TURN log:\n${log.slice(-1500)}`);
    if (!/user <nerp-test>/.test(log)) throw new Error('the test account was not used');
  });

  await step('leaving ends the call', async () => {
    await annP.click('button:has-text("Leave")');
    await annP.waitForURL(/\/messenger\?chat=/);
    await bob.click('button:has-text("Leave")');
    await bob.waitForURL(/\/messenger\?chat=/);
  });
} finally {
  await browser.close();
  turn.stop();
  await cleanupRun(s);
}
console.log(errors.length ? 'console errors:\n' + errors.join('\n') : 'no console errors');
process.exit(failed || errors.length ? 1 : 0);
