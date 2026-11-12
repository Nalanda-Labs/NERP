// SAML sign-on in the browser: an administrator sets it up on Directory & sign-on (from the
// stand-in identity provider's metadata address), and an employee signs in with the button on
// the login page — through the identity provider's self-posting form — and lands in the app.
import { chromium } from 'playwright';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { OUTPUT } from './output.ts';
import { IDP, startMockIdp } from './mock-saml-idp.ts';
const B = 'http://localhost:8080';
const s = Date.now().toString(36);
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: string[] = [];
let failed = 0, n = 0;
const pages: import('playwright').Page[] = [];
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `${OUTPUT}/fail75-${n}-${i}.png`, fullPage: true }).catch(() => {}); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const before = (await admin.get('/directory')).data.saml?.config ?? {};
const idp = await startMockIdp();
const email = `sam.${s}@acme.test`;
await admin.post('/users', { email, password: 'password123', first_name: 'Sam', last_name: `Saml${s}` });

const ctx = await browser.newContext({ viewport: { width: 1300, height: 900 }, timezoneId: 'UTC', locale: 'en-GB' });
const pa = await ctx.newPage();
pa.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
pa.on('console', (m) => m.type() === 'error' && !/400|401|403|404|Failed to load resource/.test(m.text()) && pa.url().startsWith(B) && errors.push(m.text() + ' @ ' + pa.url()));
pages.push(pa);

try {
  await step('set up on Directory & sign-on', async () => {
    await pa.goto(B + '/login'); await pa.fill('#email', 'admin@acme.test'); await pa.fill('#password', 'supersecret1');
    await pa.click('button:has-text("Sign in")'); await pa.waitForURL(B + '/');
    await pa.goto(B + '/settings/directory');
    const form = pa.locator('[data-saml-settings]');
    await form.waitFor();
    if (!(await form.locator('[data-saml-address="Entity ID"]').innerText()).endsWith('/api/auth/saml/metadata')) throw new Error('no entity id');
    await form.locator('[data-saml-metadata-url]').fill(`${IDP}/metadata`);
    await form.locator('#saml-label').fill(`Company login ${s}`);
    await form.locator('[data-saml-enabled]').check();
    await form.locator('button:has-text("Save")').click();
    await form.locator('[data-saml-idp]', { hasText: 'Identity provider:' }).waitFor();
  });

  await step('an employee signs in with the button', async () => {
    await ctx.clearCookies();
    idp.next({ email, first: 'Sam', last: 'Saml' });
    await pa.goto(B + '/login');
    await pa.click(`[data-saml]:has-text("Company login ${s}")`);
    await pa.waitForURL(B + '/', { timeout: 20000 });
    await pa.locator(`text=Sam`).first().waitFor();
    const me = await pa.evaluate(async () => {
      const r = await fetch('/api/auth/refresh', { method: 'POST' });
      const t = (await r.json()).access_token;
      return (await (await fetch('/api/auth/me', { headers: { authorization: `Bearer ${t}` } })).json());
    });
    if ((me.email ?? me.user?.email) !== email) throw new Error(`signed in as ${JSON.stringify(me).slice(0, 200)}`);
  });

  await step('a refused answer shows on the login page', async () => {
    await ctx.clearCookies();
    idp.next({ email, mode: 'unsigned' });
    await pa.goto(B + '/login');
    await pa.click('[data-saml]');
    await pa.waitForURL(/\/login/, { timeout: 20000 });
    await pa.locator('text=could not be verified').waitFor();
  });

  await step('no browser errors', async () => { if (errors.length) throw new Error(errors.join(' | ')); });
} finally {
  await browser.close();
  await admin.put('/directory/saml', { enabled: false, metadata_url: before.metadata_url ?? '', metadata_xml: before.metadata_xml ?? '', button_label: before.button_label ?? '', allowed_domains: before.allowed_domains ?? [], auto_create: before.auto_create ?? false, email_attribute: before.email_attribute ?? '' }).catch(() => {});
  await idp.close();
  await cleanupRun(s);
}
console.log(failed ? `\n${failed} failed` : '\nall passed');
process.exit(failed ? 1 : 0);
