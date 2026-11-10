import { chromium } from 'playwright';
import crypto from 'node:crypto';
import { cleanupRun } from './cleanup.ts';
const B = 'http://localhost:8080', MP = 'http://localhost:8026/api/v1';
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: any[] = [];
const ctx = async () => {
  const c = await browser.newContext({ viewport: { width: 1360, height: 860 } });
  const p = await c.newPage();
  p.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
  p.on('console', (m) => m.type() === 'error' && !/401|Failed to load resource/.test(m.text()) && errors.push(m.text()));
  return p;
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let n = 0;
const step = async (name, page, fn) => { try { await fn(); console.log('ok  ', name); } catch (e: any) { console.log('FAIL', name, e.message.split('\n')[0]); await page.screenshot({ path: `fail-${++n}.png` }); } };
async function mailLink(to, subj, path) {
  for (let i = 0; i < 40; i++) {
    const r = await (await fetch(`${MP}/search?query=${encodeURIComponent('to:' + to)}&limit=5`)).json();
    const m = r.messages?.find((x) => x.Subject.includes(subj));
    if (m) { const full = await (await fetch(`${MP}/message/${m.ID}`)).json(); return B + '/' + path + full.Text.match(new RegExp(path + '(\\?token=[\\w-]+)'))[1]; }
    await sleep(250);
  }
  throw new Error('no mail');
}
function totp(secretB32, t = Date.now() / 1000) {
  const alpha = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ234567'; let bits = '';
  for (const c of secretB32.replace(/=+$/, '')) bits += alpha.indexOf(c).toString(2).padStart(5, '0');
  const key = Buffer.from(bits.match(/.{8}/g)!.map((b) => parseInt(b, 2)));
  const ctr = Buffer.alloc(8); ctr.writeBigUInt64BE(BigInt(Math.floor(t / 30)));
  const h = crypto.createHmac('sha1', key).update(ctr).digest(); const o = h[19] & 15;
  return String((h.readUInt32BE(o) & 0x7fffffff) % 1e6).padStart(6, '0');
}
const s = Date.now().toString(36);
const email = `erin.${s}@acme.test`;
let erinSecret = '';

const admin = await ctx();
await step('admin login + live dot', admin, async () => {
  await admin.goto(B + '/login'); await admin.fill('#email', 'admin@acme.test'); await admin.fill('#password', 'supersecret1');
  await admin.click('button:has-text("Sign in")'); await admin.waitForURL(B + '/');
  await admin.waitForSelector('button[aria-label^="Notifications"] span[title="Live updates on"]');
});
await step('invite via UI', admin, async () => {
  await admin.goto(B + '/employees'); await admin.click('button:has-text("Add employee")');
  await admin.fill('#fn', 'Erin'); await admin.fill('#ln', 'Evans'); await admin.fill('#em', email);
  await admin.screenshot({ path: '01-invite-modal.png' });
  await admin.click('dialog button:has-text("Send invitation")');
  await admin.waitForSelector('.badge:has-text("Invitation pending")'); await admin.waitForSelector('button:has-text("Resend invitation")');
});
const erin = await ctx();
await step('accept invite from email link', erin, async () => {
  await erin.goto(await mailLink(email, 'invited you', 'accept-invite'));
  await erin.waitForSelector('h1:has-text("Join Acme Ltd")'); await erin.screenshot({ path: '02-accept-invite.png' });
  await erin.fill('#pw', 'erinpassword1'); await erin.fill('#pw2', 'erinpassword1'); await erin.click('button:has-text("Join and sign in")');
  await erin.waitForURL(B + '/profile');
});
await step('admin gets live toast + bell badge', admin, async () => {
  await admin.waitForSelector('text=Erin Evans accepted your invitation');
  await admin.waitForSelector('button[aria-label*="unread"]');
  await admin.click('button[aria-label^="Notifications"]'); await admin.screenshot({ path: '03-bell.png' });
  await admin.click('text=Mark all as read'); await admin.waitForSelector('button[aria-label="Notifications"]');
  await admin.keyboard.press('Escape');
});
await step('erin enables 2FA', erin, async () => {
  await erin.waitForSelector('text=Two-factor authentication'); await erin.click('section:has-text("Two-factor") button:has-text("Turn on")');
  await erin.fill('dialog input[type=password]', 'erinpassword1'); await erin.click('dialog button:has-text("Continue")');
  await erin.waitForSelector('dialog img[alt^="QR"]'); await erin.screenshot({ path: '04-2fa-qr.png' });
  const secret = (await erin.textContent('dialog p.font-mono'))!.trim();
  erinSecret = secret;
  await erin.fill('dialog input[inputmode=numeric]', totp(secret)); await erin.click('dialog button:has-text("Verify and turn on")');
  await erin.waitForSelector('text=Save these recovery codes'); await erin.screenshot({ path: '05-2fa-codes.png' });
  await erin.click('button:has-text("I\'ve saved my codes")'); await erin.waitForSelector('text=On since');
});
await step('erin signs in with 2FA', erin, async () => {
  await erin.click('header button:has-text("Erin Evans")'); await erin.click('text=Sign out'); await erin.waitForURL(/login/);
  await erin.fill('#email', email); await erin.fill('#password', 'erinpassword1'); await erin.click('button:has-text("Sign in")');
  await erin.waitForSelector('h1:has-text("Two-factor authentication")'); await erin.screenshot({ path: '06-2fa-login.png' });
  await erin.fill('#code', totp(erinSecret, Date.now() / 1000 + 30)); await erin.click('button:has-text("Verify")');
  await erin.waitForURL(B + '/');
});
const erin2 = await ctx();
await step('second device + sessions list', erin, async () => {
  await erin2.goto(B + '/login'); await erin2.fill('#email', email); await erin2.fill('#password', 'erinpassword1'); await erin2.click('button:has-text("Sign in")');
  await erin2.waitForSelector('#code'); await sleep(31000); // fresh time step (codes are single-use)
  await erin2.fill('#code', totp(erinSecret, Date.now() / 1000 + 30)); await erin2.click('button:has-text("Verify")'); await erin2.waitForURL(B + '/');
  await erin.goto(B + '/profile'); await erin.waitForSelector('text=This device');
  await erin.locator('#security').scrollIntoViewIfNeeded(); await erin.screenshot({ path: '07-sessions.png', fullPage: true });
});
await step('sign out other device -> it is kicked live', erin2, async () => {
  await erin.click('section:has-text("Where you are signed in") button:has-text("Sign out") >> nth=1');
  await erin2.waitForURL(/login/, { timeout: 10000 });
});
await step('admin turns off erin 2FA + system page', admin, async () => {
  await admin.goto(B + '/employees'); await admin.fill('input[placeholder^="Search"]', email); await admin.waitForTimeout(600); await admin.click('tbody >> text=Erin Evans');
  await admin.waitForSelector('.badge:has-text("2FA on")');
  admin.once('dialog', (d) => d.accept()); await admin.click('button:has-text("Turn off 2FA")'); await admin.waitForSelector('text=Two-factor authentication turned off');
  await admin.screenshot({ path: '08-employee-admin.png', fullPage: true });
  await admin.goto(B + '/system'); await admin.waitForSelector('text=email.send'); await admin.click('button:has-text("Send test")');
  await admin.waitForSelector('text=Test email queued'); await admin.screenshot({ path: '09-system.png', fullPage: true });
});
await step('erin sees admin notification live', erin, async () => { await erin.waitForSelector('text=Two-factor authentication was turned off by an administrator'); });
const anon = await ctx();
await step('forgot + reset password via email', anon, async () => {
  await anon.goto(B + '/login'); await anon.click('text=Forgot password?'); await anon.waitForURL(/forgot-password/); await anon.fill('#email', email); await anon.click('button:has-text("Send reset link")');
  await anon.waitForSelector('text=Check your email');
  await anon.goto(await mailLink(email, 'Reset your', 'reset-password'));
  await anon.fill('#pw', 'erinNEWpass9'); await anon.fill('#pw2', 'erinNEWpass9'); await anon.click('button:has-text("Set new password")');
  await anon.waitForSelector('text=Password changed');
});
await step('reset kicked erin out live', erin, async () => { await erin.waitForURL(/login/, { timeout: 10000 }); });
await step('notifications page', admin, async () => { await admin.goto(B + '/notifications'); await admin.waitForSelector('h1:has-text("Notifications")'); await admin.screenshot({ path: '10-notifications.png' }); });
console.log('errors:', errors.length ? errors : 'none');
await browser.close();
await cleanupRun(s);
