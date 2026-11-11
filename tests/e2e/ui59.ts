// HR documents in the browser: HR writes a template and sends it to an employee with a
// company signer; the signer and then the employee draw their signatures and sign; the
// signed document shows as signed with its fingerprint; HR declares it invalid.
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
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `fail59-${n}-${i}.png`, fullPage: true }); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
for (const first of ['Cleo', 'Ezra']) await admin.post('/users', { email: `${first.toLowerCase()}.${s}@acme.test`, password: 'password123', first_name: first, last_name: `Doc${s}` });

const ctx = async (email: string, password = 'password123') => {
  const p = await (await browser.newContext({ viewport: { width: 1440, height: 1000 }, timezoneId: 'UTC', locale: 'en-GB' })).newPage();
  p.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
  p.on('console', (m) => m.type() === 'error' && !/401|400|404|409|Failed to load resource/.test(m.text()) && errors.push(m.text()));
  pages.push(p);
  await p.goto(B + '/login'); await p.fill('#email', email); await p.fill('#password', password);
  await p.click('button:has-text("Sign in")'); await p.waitForURL(B + '/');
  return p;
};
const signHere = async (p: import('playwright').Page) => {
  await p.locator('canvas[aria-label="Draw your signature"]').waitFor();
  const box = (await p.locator('canvas[aria-label="Draw your signature"]').boundingBox())!;
  await p.mouse.move(box.x + 30, box.y + 60); await p.mouse.down();
  await p.mouse.move(box.x + 120, box.y + 30, { steps: 6 }); await p.mouse.move(box.x + 220, box.y + 80, { steps: 6 }); await p.mouse.up();
  await p.check('[aria-label="Sign"] input[type="checkbox"]');
  await p.click('[aria-label="Sign"] button:has-text("Sign")');
};

try {
  const ada = await ctx('admin@acme.test', 'supersecret1');
  const cleo = await ctx(`cleo.${s}@acme.test`);
  const ezra = await ctx(`ezra.${s}@acme.test`);

  await step('HR writes a template and sends it', async () => {
    await ada.goto(B + '/hr/documents');
    await ada.click('[role="tab"]:has-text("Templates")');
    await ada.click('button:has-text("New template")');
    await ada.fill('#tp-name', `NDA ${s}`);
    await ada.fill('#tp-body', `# Confidentiality\n\n{{name}} keeps {{company}}'s secrets.`);
    await ada.click('button[form="tpl-form"]');
    await ada.locator(`[data-template="NDA ${s}"]`).waitFor();
    await ada.click('button:has-text("Send documents")');
    await ada.fill('#sd-title', `NDA ${s}`);
    await ada.selectOption('#sd-src', { label: `Template: NDA ${s}` });
    await ada.fill('dialog input[placeholder="Add employees…"]', `Ezra Doc${s}`);
    await ada.click(`dialog button:has-text("Ezra Doc${s}")`);
    await ada.fill('dialog input[placeholder="Company signer…"]', `Cleo Doc${s}`);
    await ada.click(`dialog button:has-text("Cleo Doc${s}")`);
    await ada.click('button[form="send-form"]');
    await ada.locator(`[data-doc="NDA ${s}"]`).waitFor();
  });

  await step('the company signer signs, then the employee', async () => {
    await cleo.goto(B + '/hr/documents');
    await cleo.click(`[data-doc="NDA ${s}"]`);
    await cleo.locator('iframe[data-pdf]').waitFor();
    await signHere(cleo);
    await cleo.locator('[data-step="company"] >> text=Done').waitFor();
    await ezra.goto(B + '/hr/documents');
    await ezra.click(`[data-doc="NDA ${s}"]`);
    await signHere(ezra);
    await ezra.locator('[data-status="signed"]').waitFor();
    await ezra.locator('[data-signed-sha]').waitFor();
  });

  await step('HR declares it invalid', async () => {
    await ada.goto(B + '/hr/documents');
    await ada.click('[role="tab"]:has-text("All")');
    await ada.click(`[data-doc="NDA ${s}"]`);
    ada.once('dialog', (d) => d.accept('Superseded'));
    await ada.click('button:has-text("Declare invalid")');
    await ada.locator('[data-status="revoked"]').waitFor();
    await ada.locator('text=Not valid: Superseded').waitFor();
  });

  await step('no browser errors', async () => { if (errors.length) throw new Error(errors.join(' | ')); });
} finally {
  await browser.close();
  await cleanupRun(s);
}
console.log(failed ? `\n${failed} failed` : '\nall passed');
process.exit(failed ? 1 : 0);
