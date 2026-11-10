// Required two-step sign-in in the browser: an administrator turns it on in Settings; an
// employee signing in is asked for their mobile number, scans the QR code (here: reads the
// key), enters a code, saves the recovery codes and gets in; next time the code is asked for.
import { chromium } from 'playwright';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { totp } from './totp.ts';
const B = 'http://localhost:8080';
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: string[] = [];
const watch = (p: import('playwright').Page) => {
  p.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
  p.on('console', (m) => m.type() === 'error' && !/401|400|404|Failed to load resource/.test(m.text()) && errors.push(m.text()));
  return p;
};
const newPage = async () => watch(await (await browser.newContext({ viewport: { width: 1280, height: 900 } })).newPage());
const s = Date.now().toString(36);
let n = 0, failed = 0;
const page = await newPage();
let shot = page;
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); await shot.screenshot({ path: `fail39-${++n}.png`, fullPage: true }); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const settings = (await admin.get('/settings')).data;
const email = `hana.${s}@acme.test`;
await admin.post('/users', { email, password: 'password123', first_name: 'Hana', last_name: 'Twostep' });
const signIn = async (p: import('playwright').Page) => {
  await p.goto(B + '/login'); await p.fill('#email', email); await p.fill('#password', 'password123');
  await p.click('button:has-text("Sign in")');
};
let secret = '';

try {
  await page.goto(B + '/login'); await page.fill('#email', 'admin@acme.test'); await page.fill('#password', 'supersecret1');
  await page.click('button:has-text("Sign in")'); await page.waitForURL(B + '/');

  await step('an administrator requires two-step sign-in', async () => {
    await page.click('aside a[href="/settings"]');
    await page.check('[data-require-2fa]');
    await page.click('button:has-text("Save settings")');
    await page.locator('text=Settings saved').waitFor();
    if ((await admin.get('/settings')).data.require_two_factor !== true) throw new Error('not saved');
  });

  const hana = await newPage();
  shot = hana;
  await step('at sign-in: mobile number, QR code, code', async () => {
    await signIn(hana);
    await hana.locator('h1:has-text("Set up two-step sign-in")').waitFor();
    await hana.locator('img[alt="QR code for your authenticator app"]').waitFor();
    secret = (await hana.locator('[data-secret]').textContent())!.trim();
    await hana.fill('#mobile', '+44 7700 900456');
    await hana.fill('#setup-code', totp(secret));
    await hana.screenshot({ path: '60-two-step-setup.png' });
    await hana.click('button:has-text("Turn on and sign in")');
    await hana.locator('[data-recovery-codes] li').nth(9).waitFor();
  });

  await step('recovery codes, then in', async () => {
    await hana.click('button:has-text("I\'ve saved them")');
    await hana.waitForURL(B + '/');
    await hana.locator('header >> text=Hana Twostep').waitFor();
    const me = await hana.evaluate(async () => (await fetch('/api/auth/refresh', { method: 'POST' })).json());
    if (me.user.mobile !== '+44 7700 900456') throw new Error('mobile not saved: ' + JSON.stringify(me.user.mobile));
  });

  const again = await newPage();
  shot = again;
  await step('next time the code is asked for', async () => {
    await signIn(again);
    await again.locator('text=Enter the 6-digit code from your authenticator app.').waitFor();
    await again.fill('#code', totp(secret, Date.now() / 1000 + 30));
    await again.click('button:has-text("Verify")');
    await again.waitForURL(B + '/');
  });
} finally {
  await admin.put('/settings', { ...settings, require_two_factor: false });
  await cleanupRun(s);
}
console.log(errors.length ? 'console errors:\n' + errors.join('\n') : 'no console errors');
await browser.close();
process.exit(failed || errors.length ? 1 : 0);
