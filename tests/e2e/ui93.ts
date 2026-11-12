// Sandbox in the browser: refreshing the copy from System → Sandbox, then a sandbox server's
// pages carry the banner.
import { chromium } from 'playwright';
import { spawn } from 'node:child_process';
import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { OUTPUT } from './output.ts';
import { dbUrl } from '../db/cleanup.ts';
import pg from 'pg';
const B = 'http://localhost:8080';
const SB = 'http://127.0.0.1:8092';
const s = Date.now().toString(36);
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: string[] = [];
let failed = 0, n = 0;
const pages: import('playwright').Page[] = [];
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `${OUTPUT}/fail93-${n}-${i}.png`, fullPage: true }).catch(() => {}); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
const sandboxUrl = dbUrl!.replace(/\/([^/?]+)(\?.*)?$/, '/$1_sandbox_test$2');
let server: ReturnType<typeof spawn> | null = null;

const ctx = await browser.newContext({ viewport: { width: 1400, height: 950 }, timezoneId: 'UTC', locale: 'en-GB' });
const pa = await ctx.newPage();
pa.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
pa.on('console', (m) => m.type() === 'error' && !/400|401|403|404|Failed to load resource/.test(m.text()) && errors.push(m.text()));
pages.push(pa);
await pa.goto(B + '/login'); await pa.fill('#email', 'admin@acme.test'); await pa.fill('#password', 'supersecret1');
await pa.click('button:has-text("Sign in")'); await pa.waitForURL(B + '/');

try {
  await step('refreshing the sandbox from System', async () => {
    await pa.goto(`${B}/system`);
    await pa.click('a[href="/system/sandbox"]');
    await pa.click('[data-sandbox-refresh]');
    // This refresh, not a previous one: it starts, then finishes.
    await pa.locator('[data-sandbox-refresh]:has-text("Refreshing")').waitFor();
    await pa.locator('[data-sandbox-refresh]:has-text("Refresh the sandbox")').waitFor({ timeout: 300000 });
    await pa.locator('[data-sandbox-last="ok"]').waitFor();
  });
  await step('the live system has no sandbox banner', async () => { await pa.goto(B + '/'); await sleep(1000); if (await pa.locator('[data-sandbox-banner]').count()) throw new Error('banner on the live system'); });

  await step('the sandbox\'s pages carry the banner', async () => {
    server = spawn('./target/debug/nerp', [], {
      cwd: new URL('../../backend/', import.meta.url).pathname,
      env: { ...process.env, SANDBOX: 'true', DATABASE_URL: sandboxUrl, BIND_ADDR: '127.0.0.1:8092', PUBLIC_URL: SB, REDIS_URL: 'redis://127.0.0.1:6379/1', UPLOAD_DIR: mkdtempSync(join(tmpdir(), 'nerp-sb-')), STATIC_DIR: '../frontend/build', COOKIE_SECURE: 'false', SANDBOX_DATABASE_URL: '' },
      stdio: 'ignore',
    });
    let up = false;
    for (let i = 0; i < 120 && !up; i++) { up = await fetch(`${SB}/api/health`).then((x) => x.ok, () => false); if (!up) await sleep(500); }
    if (!up) throw new Error('the sandbox server did not start');
    const sp = await (await browser.newContext({ viewport: { width: 1200, height: 900 } })).newPage();
    pages.push(sp);
    await sp.goto(SB + '/login'); await sp.fill('#email', 'admin@acme.test'); await sp.fill('#password', 'supersecret1');
    await sp.click('button:has-text("Sign in")'); await sp.waitForURL(SB + '/');
    await sp.locator('[data-sandbox-banner]').waitFor();
    await sp.goto(`${SB}/system/sandbox`);
    await sp.locator('[data-outbox]').waitFor();
  });

  await step('no browser errors', async () => { if (errors.length) throw new Error(errors.join(' | ')); });
} finally {
  await browser.close();
  if (server) { (server as any).kill('SIGTERM'); await sleep(1500); }
  await db.query(`DROP DATABASE IF EXISTS "${sandboxUrl.replace(/^.*\/([^/?]+)(\?.*)?$/, '$1')}" WITH (FORCE)`).catch(() => {});
  await db.end();
  await cleanupRun(s);
}
console.log(failed ? `\n${failed} failed` : '\nall passed');
process.exit(failed ? 1 : 0);
