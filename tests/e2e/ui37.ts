// Directory and sign-on in the browser: the settings page (LDAP connection test and sync,
// sign-on setup), signing in with a directory password, and the sign-on button round trip
// through a mock provider (success, and a refusal shown on the login page).
import { chromium } from 'playwright';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { BASE_DN, LDAP_PORT, SERVICE_DN, SERVICE_PASSWORD, directorySkipReason, ldapReady, startLdap } from './ldap-server.ts';
import { startMockOidc } from './mock-oidc.ts';
const B = 'http://localhost:8080';
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const skip = await directorySkipReason(admin, B);
if (skip) { console.log(`skipped: ${skip}`); process.exit(0); }
const browser = await chromium.launch({ executablePath: exe });
const errors: string[] = [];
const watch = (p: import('playwright').Page) => {
  p.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
  p.on('console', (m) => m.type() === 'error' && !/401|400|404|Failed to load resource/.test(m.text()) && errors.push(m.text()));
  return p;
};
const page = watch(await (await browser.newContext({ viewport: { width: 1440, height: 1000 } })).newPage());
const s = Date.now().toString(36);
let n = 0, failed = 0;
let shot = page;
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); await shot.screenshot({ path: `fail37-${++n}.png`, fullPage: true }); }
};
const stop = startLdap(`nerp-e2e-ldap-${s}`, [
  { uid: `erin${s}`, given: 'Erin', sn: 'Directory', email: `erin.${s}@acme.test`, password: 'erin-pass-1', title: 'Analyst' },
  { uid: `finn${s}`, given: 'Finn', sn: 'Directory', email: `finn.${s}@acme.test`, password: 'finn-pass-1' },
]);
const oidc = await startMockOidc(9478);
await page.goto(B + '/login'); await page.fill('#email', 'admin@acme.test'); await page.fill('#password', 'supersecret1');
await page.click('button:has-text("Sign in")'); await page.waitForURL(B + '/');

const ldapOn = (await admin.get('/directory')).data.ldap_available === true;
try {
  if (!ldapOn) {
    await step('LDAP off on this server: the page says so', async () => {
      await page.click('aside a:has-text("Directory & sign-on")');
      await page.locator('[data-ldap-off] >> text=LDAP_ENABLED=true').waitFor();
    });
  } else {
  await ldapReady();
  await step('directory settings: connect and test', async () => {
    await page.click('aside a:has-text("Directory & sign-on")');
    await page.locator('#ldap-url').waitFor();
    await page.fill('#ldap-url', `ldap://127.0.0.1:${LDAP_PORT}`);
    await page.fill('#ldap-bind', SERVICE_DN);
    await page.fill('#ldap-pass', SERVICE_PASSWORD);
    await page.fill('#ldap-base', BASE_DN);
    await page.fill('#ldap-filter', '(objectClass=inetOrgPerson)');
    await page.check('[data-ldap-enabled]');
    await page.click('[data-ldap] button:has-text("Save")');
    await page.locator('#ldap-pass[placeholder^="Saved"]').waitFor();
    await page.click('button:has-text("Test connection")');
    await page.locator('[data-ldap-test] >> text=2 employees found').waitFor();
    await page.locator(`[data-ldap-test] >> text=Erin Directory <erin.${s}@acme.test>`).waitFor();
  });

  await step('sync now', async () => {
    await page.click('button:has-text("Sync now")');
    await page.locator('text=Synced: 2 added, 0 updated, 0 deactivated').waitFor();
    await page.locator('[data-ldap-last] >> text=2 found, 2 added').waitFor();
  });

  }

  await step('sign-on settings', async () => {
    if (!ldapOn) await page.locator('#sso-issuer').waitFor();
    await page.fill('#sso-issuer', oidc.issuer);
    await page.fill('#sso-client', oidc.clientId);
    await page.fill('#sso-secret', oidc.clientSecret);
    await page.fill('#sso-label', `Sign in with Acme ${s}`);
    await page.fill('#sso-domains', 'acme.test');
    await page.check('[data-sso-settings] input[type=checkbox] >> nth=1');
    await page.check('[data-sso-enabled]');
    await page.click('[data-sso-settings] button:has-text("Save")');
    await page.locator('#sso-secret[placeholder^="Saved"]').waitFor();
    await page.locator('[data-redirect-uri] >> text=http://localhost:8080/api/auth/sso/callback').waitFor();
    await page.click('button:has-text("Check provider")');
    await page.locator('[data-sso-test] >> text=The provider answered.').waitFor();
  });
  await page.screenshot({ path: '57-directory.png', fullPage: true });

  const fresh = async () => { const p = watch(await (await browser.newContext({ viewport: { width: 1280, height: 900 } })).newPage()); shot = p; return p; };
  if (ldapOn) await step('a directory employee signs in with the directory password', async () => {
    const p = await fresh();
    await p.goto(B + '/login');
    await p.fill('#email', `erin.${s}@acme.test`); await p.fill('#password', 'erin-pass-1');
    await p.click('button:has-text("Sign in")'); await p.waitForURL(B + '/');
    await p.locator('header >> text=Erin Directory').waitFor();
    await p.context().close();
  });

  await step('the sign-on button', async () => {
    const p = await fresh();
    oidc.as = { sub: `sub-gil-${s}`, email: `gil.${s}@acme.test`, given_name: 'Gil', family_name: 'Signon', email_verified: true };
    await p.goto(B + '/login');
    await p.screenshot({ path: '58-login-sso.png' });
    await p.click(`a[data-sso]:has-text("Sign in with Acme ${s}")`);
    await p.waitForURL(B + '/');
    await p.locator('header >> text=Gil Signon').waitFor();
    await p.reload();
    await p.locator('header >> text=Gil Signon').waitFor();
    await p.context().close();
  });

  await step('a refused sign-on is explained on the login page', async () => {
    const p = await fresh();
    oidc.deny = 'You are not assigned to this application';
    await p.goto(B + '/login');
    await p.click('a[data-sso]');
    await p.waitForURL(/\/login/);
    await p.locator('text=You are not assigned to this application').waitFor();
    if (p.url().includes('sso_error')) throw new Error('the error stays in the address bar');
    await p.context().close();
  });
} finally {
  shot = page;
  await admin.put('/directory/ldap', { enabled: false, bind_password: '' });
  await admin.put('/directory/sso', { enabled: false, issuer: '', client_id: '', client_secret: '' });
  stop();
  await oidc.close();
  await cleanupRun(s);
}
console.log(errors.length ? 'console errors:\n' + errors.join('\n') : 'no console errors');
await browser.close();
process.exit(failed || errors.length ? 1 : 0);
