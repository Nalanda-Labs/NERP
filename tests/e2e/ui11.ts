// Gmail sign-in round trip in the browser, against the mock OAuth provider.
import { chromium } from 'playwright';
import { cleanupRun, requireOAuthMock } from './cleanup.ts';
// Skip before creating anything when the server has no mock OAuth provider.
await requireOAuthMock();
const B = 'http://localhost:8080';
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: any[] = [];
const ctx = await browser.newContext({ viewport: { width: 1440, height: 1000 } });
const page = await ctx.newPage();
page.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
page.on('console', (m) => m.type() === 'error' && !/401|400|Failed to load resource/.test(m.text()) && errors.push(m.text()));
let n = 0, failed = 0;
const step = async (name, fn) => { try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); await page.screenshot({ path: `fail11-${++n}.png`, fullPage: true }); } };
const s = Date.now().toString(36);

import http from 'node:http';
import { createHash } from 'node:crypto';
const b64u = (b) => Buffer.from(b).toString('base64url');
const codes = new Map();
const mock = http.createServer(async (req, res) => {
  const u = new URL(req.url!, 'http://127.0.0.1:8099');
  if (u.pathname.endsWith('/authorize')) {
    const code = 'c' + Math.random().toString(36).slice(2);
    codes.set(code, u.searchParams.get('code_challenge'));
    // A tiny consent page, like the provider's.
    res.writeHead(200, { 'content-type': 'text/html' });
    return res.end(`<h1>Mock Google</h1><a id="allow" href="${u.searchParams.get('redirect_uri')}?code=${code}&state=${u.searchParams.get('state')}">Allow</a>`);
  }
  let body = ''; for await (const c of req) body += c;
  const f = new URLSearchParams(body);
  res.writeHead(200, { 'content-type': 'application/json' });
  if (f.get('grant_type') === 'refresh_token') return res.end(JSON.stringify({ access_token: 'at2', expires_in: 3600 }));
  const ok = createHash('sha256').update(f.get('code_verifier') ?? '').digest('base64url') === codes.get(f.get('code'));
  res.end(JSON.stringify(ok ? { access_token: 'at', refresh_token: 'rt', expires_in: 3600, id_token: `${b64u('{}')}.${b64u(JSON.stringify({ email: `ui.${s}@gmail.example`, name: 'Gina' }))}.x` } : { error: 'invalid_grant' }));
});
await new Promise<void>((r) => mock.listen(8099, '127.0.0.1', () => r()));

await page.goto(B + '/login'); await page.fill('#email', 'admin@acme.test'); await page.fill('#password', 'supersecret1');
await page.click('button:has-text("Sign in")'); await page.waitForURL(B + '/');

await step('Connect Gmail → consent → back with the mailbox', async () => {
  await page.goto(B + '/crm/mailboxes');
  await page.click('button:has-text("Connect Gmail")');
  await page.waitForSelector('h1:text-is("Mock Google")');
  await page.click('#allow');
  await page.waitForURL(B + '/crm/mailboxes');
  await page.waitForSelector(`text=ui.${s}@gmail.example connected`);
  await page.waitForSelector(`li:has-text("ui.${s}@gmail.example") >> text=Google`);
  await page.screenshot({ path: '23-gmail-connected.png' });
});

await step('OAuth mailbox settings show options only', async () => {
  await page.click(`li:has-text("ui.${s}@gmail.example") >> button:has-text("Edit")`);
  await page.waitForSelector('text=signed in with Google');
  if (await page.locator('#mb-ih').count()) throw new Error('server fields shown');
  await page.check('text=Create a lead when someone unknown writes');
  await page.click('dialog >> button:text-is("Save")');
  await page.waitForSelector(`li:has-text("ui.${s}@gmail.example") >> text=new senders become leads`);
});

await step('system page shows sign-in configuration', async () => {
  await page.goto(B + '/system');
  await page.waitForSelector('text=Google: configured');
});

await step('cleanup', async () => {
  page.on('dialog', (d) => d.accept());
  await page.goto(B + '/crm/mailboxes');
  await page.click(`button[aria-label="Disconnect ui.${s}@gmail.example"]`);
  await page.waitForSelector(`text=ui.${s}@gmail.example`, { state: 'detached' });
});

mock.close();
console.log(errors.length ? 'CONSOLE ERRORS:\n' + errors.join('\n') : 'no console errors');
await browser.close();
await cleanupRun(s);
process.exit(failed || errors.length ? 1 : 0);
