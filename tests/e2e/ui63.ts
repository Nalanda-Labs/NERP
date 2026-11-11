// Signing in with another account in the browser: an administrator adds Facebook under
// Directory & sign-on, the login page shows the button, an employee signs in with it (a
// stand-in Facebook on :9479 answers at once) and sees the account on their profile.
import http from 'node:http';
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
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `fail63-${n}-${i}.png`, fullPage: true }); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const email = `fay.${s}@acme.test`;
await admin.post('/users', { email, password: 'password123', first_name: 'Fay', last_name: `Book${s}` });

// Facebook, standing in: the dialog sends the browser straight back with a code.
const mock = http.createServer((req, res) => {
  const url = new URL(req.url!, 'http://127.0.0.1:9479');
  if (url.pathname === '/www.facebook.com/v23.0/dialog/oauth') {
    const back = new URL(url.searchParams.get('redirect_uri')!);
    back.searchParams.set('code', 'fb-code');
    back.searchParams.set('state', url.searchParams.get('state')!);
    res.writeHead(302, { location: back.toString() });
    return res.end();
  }
  res.writeHead(200, { 'content-type': 'application/json' });
  if (url.pathname === '/graph.facebook.com/v23.0/oauth/access_token') return res.end(JSON.stringify({ access_token: 'fb-token' }));
  if (url.pathname === '/graph.facebook.com/v23.0/me') return res.end(JSON.stringify({ id: `fb-${s}`, name: 'Fay', email }));
  res.end('{}');
});
await new Promise<void>((r) => mock.listen(9479, '127.0.0.1', r));

const ctx = async () => {
  const p = await (await browser.newContext({ viewport: { width: 1280, height: 900 }, timezoneId: 'UTC', locale: 'en-GB' })).newPage();
  p.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
  p.on('console', (m) => m.type() === 'error' && !/401|400|403|404|409|Failed to load resource/.test(m.text()) && errors.push(m.text()));
  pages.push(p);
  return p;
};

try {
  const ada = await ctx();
  await ada.goto(B + '/login'); await ada.fill('#email', 'admin@acme.test'); await ada.fill('#password', 'supersecret1');
  await ada.click('button:has-text("Sign in")'); await ada.waitForURL(B + '/');

  await step('add Facebook under Directory & sign-on', async () => {
    await ada.goto(B + '/settings/directory');
    await ada.locator('[data-providers] button:has-text("Add")').click();
    await ada.selectOption('#sp-kind', 'facebook');
    await ada.fill('#sp-label', `Facebook ${s}`);
    await ada.fill('#sp-client', 'fb-client');
    await ada.fill('#sp-secret', 'shh');
    await ada.click('dialog button:has-text("Save")');
    await ada.locator(`[data-provider-row="facebook"]:has-text("Facebook ${s}")`).waitFor();
    await ada.screenshot({ path: '/tmp/nerp-e2e/providers.png', fullPage: true });
  });

  await step('Fay signs in with it', async () => {
    const fay = await ctx();
    await fay.goto(B + '/login');
    await fay.click(`[data-provider="facebook"]:has-text("Facebook ${s}")`);
    await fay.waitForURL(B + '/');
    await fay.goto(B + '/profile');
    await fay.locator('[data-linked="facebook"]').waitFor();
    await fay.screenshot({ path: '/tmp/nerp-e2e/linked-accounts.png', fullPage: true });
  });

  await step('no browser errors', async () => {
    if (errors.length) throw new Error(errors.join(' | '));
  });
} finally {
  for (const p of (await admin.get('/directory/providers')).data.items ?? []) if (String(p.label).includes(s)) await admin.del(`/directory/providers/${p.id}`);
  mock.close();
  await browser.close();
  await cleanupRun(s);
}
console.log(failed ? `\n${failed} failed` : '\nall passed');
process.exit(failed ? 1 : 0);
