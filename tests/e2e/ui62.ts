// Security and administration in the browser: switching off links for anyone (a board's share
// dialog then says so), a link to join the company used by a newcomer and approved, the
// storage page, and the network list in the settings.
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
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `fail62-${n}-${i}.png`, fullPage: true }); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const board = (await admin.post('/boards', { title: `Links ${s}` })).data;

const ctx = async (email: string | null, password = 'password123') => {
  const p = await (await browser.newContext({ viewport: { width: 1440, height: 1000 }, timezoneId: 'UTC', locale: 'en-GB' })).newPage();
  p.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
  p.on('console', (m) => m.type() === 'error' && !/401|400|403|404|409|Failed to load resource/.test(m.text()) && errors.push(m.text()));
  pages.push(p);
  if (email) {
    await p.goto(B + '/login'); await p.fill('#email', email); await p.fill('#password', password);
    await p.click('button:has-text("Sign in")'); await p.waitForURL(B + '/');
  }
  return p;
};

try {
  const ada = await ctx('admin@acme.test', 'supersecret1');

  await step('switch off links for anyone in the settings', async () => {
    await ada.goto(B + '/settings');
    await ada.locator('#s-ip').waitFor();
    await ada.uncheck('[data-public-links]');
    await ada.click('button:has-text("Save")');
    await ada.locator('text=Settings saved').waitFor();
  });

  await step('a board’s share dialog says links are off', async () => {
    await ada.goto(`${B}/boards/${board.id}`);
    await ada.click('button:has-text("Share")');
    await ada.locator('dialog [data-links-off]').waitFor();
    if (await ada.locator('dialog button:has-text("Make a link")').count()) throw new Error('can still make links');
    await admin.put('/settings', { public_links: true });
  });

  let joinUrl = '';
  await step('make a link to join', async () => {
    await ada.goto(B + '/employees');
    await ada.click('a:has-text("Invitations")');
    await ada.fill('#jl-note', `Interns ${s}`);
    await ada.fill('#jl-dom', 'acme.test');
    await ada.click('button:has-text("Make a link")');
    joinUrl = (await ada.locator('[data-join-link]').textContent())!.trim();
    await ada.locator(`[data-link-row="Interns ${s}"]`).waitFor();
  });

  await step('a newcomer asks to join', async () => {
    const guest = await ctx(null);
    await guest.goto(joinUrl);
    await guest.locator('h1:has-text("Join Acme Ltd")').waitFor();
    await guest.fill('#j-email', `ivy.${s}@gmail.com`);
    await guest.fill('#j-first', 'Ivy');
    await guest.fill('#j-last', `New${s}`);
    await guest.click('button:has-text("Ask to join")');
    await guest.locator('[role="alert"]:has-text("acme.test")').waitFor();
    await guest.fill('#j-email', `ivy.${s}@acme.test`);
    await guest.click('button:has-text("Ask to join")');
    await guest.locator('[data-join-done]').waitFor();
    await guest.screenshot({ path: '/tmp/nerp-e2e/join-done.png' });
  });

  await step('approve the request', async () => {
    await ada.reload();
    const req = ada.locator(`[data-request="ivy.${s}@acme.test"]`);
    await req.waitFor();
    await ada.screenshot({ path: '/tmp/nerp-e2e/invitations.png', fullPage: true });
    await req.locator('button:has-text("Approve")').click();
    await ada.locator(`text=Ivy New${s} is invited by email`).waitFor();
  });

  await step('the storage page', async () => {
    await ada.goto(B + '/system');
    await ada.click('a:has-text("Storage")');
    await ada.locator('[data-total]').waitFor();
    await ada.locator('[data-area="drive"], [data-area="other"]').first().waitFor();
    await ada.screenshot({ path: '/tmp/nerp-e2e/storage.png', fullPage: true });
  });

  await step('the backups page', async () => {
    await ada.goto(B + '/system');
    await ada.click('a:has-text("Backups")');
    await ada.locator('button:has-text("Back up now")').waitFor();
    await ada.locator('[data-run="backup"]').waitFor();
    await ada.screenshot({ path: '/tmp/nerp-e2e/backups.png', fullPage: true });
  });

  await step('customise the menu and the background', async () => {
    ada.on('dialog', (d) => d.dismiss());
    await ada.goto(B + '/');
    await ada.click('button:has-text("Customise")');
    await ada.click('button[aria-label="Hide Boards"]');
    await ada.click('button[aria-label="Pin Deals"]');
    await ada.click('button[aria-label="Background sky"]');
    await ada.click('[data-customize] button:has-text("Done")');
    await ada.locator('[data-pinned] a:has-text("Deals")').waitFor();
    if (await ada.locator('nav a[href="/boards"]').count()) throw new Error('Boards still shown');
    await ada.locator('[data-background="sky"]').waitFor();
    await ada.screenshot({ path: '/tmp/nerp-e2e/customised.png' });
    await admin.put('/me/ui', { menu: null, background: null });
  });

  await step('the network list refuses a mistake', async () => {
    await ada.goto(B + '/settings');
    await ada.fill('#s-ip', '192.168.1.0/24\nnot-an-address');
    await ada.click('button:has-text("Save")');
    await ada.locator('text=not-an-address').last().waitFor();
  });

  await step('no browser errors', async () => {
    if (errors.length) throw new Error(errors.join(' | '));
  });
} finally {
  await admin.put('/settings', { ip_allow: [], public_links: true });
  await browser.close();
  await admin.del(`/boards/${board.id}`);
  await cleanupRun(s);
}
console.log(failed ? `\n${failed} failed` : '\nall passed');
process.exit(failed ? 1 : 0);
