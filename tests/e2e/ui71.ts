// Tracking and marketing in the browser: a company website with the tracking script and an
// embedded form (a visitor from a search engine sends it; the record shows the journey), the
// Traffic page (websites, ad links, analysis), the sales generator, and the email designer.
import { chromium } from 'playwright';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
const B = 'http://localhost:8080';
const s = Date.now().toString(36);
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
// The test's "company website" is a made-up public host loading NERP from localhost, which
// Chrome's local network protection would block; a real NERP server is not on localhost.
const browser = await chromium.launch({ executablePath: exe, args: ['--disable-features=BlockInsecurePrivateNetworkRequests,PrivateNetworkAccessSendPreflights,PrivateNetworkAccessRespectPreflightResults,LocalNetworkAccessChecks'] });
const errors: string[] = [];
let failed = 0, n = 0;
const pages: import('playwright').Page[] = [];
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `fail71-${n}-${i}.png`, fullPage: true }).catch(() => {}); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const host = `shop-${s}.example`;
let siteId = '', formId = '', campId = '', segId = '';
const cleanup: string[] = [];

try {
  const site = (await admin.post('/tracking/sites', { name: `Shop ${s}`, domains: [host] })).data;
  siteId = site.id;
  const form = (await admin.post('/crm/webforms', { name: `Site form ${s}`, entity_type: 1, fields: [{ label: 'Name', kind: 'text', required: true, maps_to: 'first_name' }, { label: 'Email', kind: 'email', required: true, maps_to: 'email' }] })).data;
  formId = form.id;

  await step('a visitor from a search engine sends the embedded form', async () => {
    const ctx = await browser.newContext({ viewport: { width: 1200, height: 900 } });
    const p = await ctx.newPage();
    pages.push(p);
    await ctx.route(`http://${host}/**`, (route) => route.fulfill({
      contentType: 'text/html',
      body: `<!doctype html><title>Spring offer</title><h1>Spring</h1><script src="${B}/track.js" data-site="${site.key}"></script><script src="${B}/embed.js" data-form="${form.key}"></script>`,
    }));
    await p.goto(`http://${host}/spring`, { referer: 'https://www.google.com/' });
    await p.waitForFunction(() => !!(window as any).NERP_VID);
    const f = p.frameLocator('iframe[title="Contact form"]');
    await f.getByLabel('Name').fill(`Walker ${s}`);
    await f.getByLabel('Email').fill(`walker.${s}@example.com`);
    await p.waitForTimeout(3500);
    await f.locator('button:has-text("Send")').click();
    await f.locator('text=/thank/i').first().waitFor({ timeout: 10000 });
    await ctx.close();
  });

  const pa = await (async () => {
    const ctx = await browser.newContext({ viewport: { width: 1400, height: 950 }, timezoneId: 'UTC', locale: 'en-GB' });
    const p = await ctx.newPage();
    p.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
    p.on('console', (m) => m.type() === 'error' && !/400|401|403|404|409|Failed to load resource/.test(m.text()) && errors.push(m.text()));
    pages.push(p);
    await p.goto(B + '/login'); await p.fill('#email', 'admin@acme.test'); await p.fill('#password', 'supersecret1');
    await p.click('button:has-text("Sign in")'); await p.waitForURL(B + '/');
    return p;
  })();

  await step('the record shows the visitor\'s journey', async () => {
    const rec = (await admin.get(`/crm/contact/items?q=walker.${s}`)).data.items[0] ?? (await admin.get(`/crm/lead/items?q=walker.${s}`)).data.items[0];
    if (!rec) throw new Error('no record');
    cleanup.push(rec.id);
    const code = (await admin.get(`/crm/contact/items/${rec.id}`)).status === 200 ? 'contact' : 'lead';
    await pa.goto(B + `/crm/${code}/${rec.id}`);
    const j = pa.locator('[data-journey]');
    await j.locator('text=First came through Search · google.com').waitFor({ timeout: 10000 });
    await j.locator('button:has-text("Search")').click();
    await j.locator('text=Spring offer').waitFor();
  });

  await step('Traffic: the analysis, websites and ad links', async () => {
    await pa.goto(B + '/marketing/traffic');
    await pa.selectOption('#t-site', siteId);
    await pa.locator('[data-traffic-report] [data-channel="search"]').waitFor();
    await pa.click('button[role="tab"]:has-text("Websites")');
    const snippet = await pa.locator(`input[aria-label="Tracking code for Shop ${s}"]`).inputValue();
    if (!snippet.includes(site.key)) throw new Error(snippet);
    await pa.click('button[role="tab"]:has-text("Ad links")');
    await pa.fill('#l-name', `Poster ${s}`);
    await pa.fill('#l-url', `https://${host}/poster`);
    await pa.fill('#l-src', `poster-${s}`);
    await pa.click('button:has-text("Make link")');
    await pa.locator(`[data-ad-link="Poster ${s}"]`).waitFor();
    const link = (await admin.get('/tracking/links')).data.items.find((x: any) => x.name === `Poster ${s}`);
    cleanup.push(`link:${link.id}`);
  });

  await step('a sales generator', async () => {
    const seg = (await admin.post('/marketing/segments', { name: `Gen seg ${s}`, entity_type: 3, filter: { q: `nobody-${s}` } })).data;
    segId = seg.id;
    await pa.goto(B + '/marketing/generators');
    await pa.click('button:has-text("New generator")');
    await pa.fill('#g-name', `Renewals ${s}`);
    await pa.selectOption('#g-seg', seg.id);
    await pa.locator('input[type="radio"][value="later"]').check();
    await pa.fill('input[aria-label="First run at"]', '2099-01-01T09:00');
    await pa.click('button:has-text("Save")');
    await pa.locator(`[data-generator="Renewals ${s}"]`).waitFor();
    const g = (await admin.get('/marketing/generators')).data.items.find((x: any) => x.name === `Renewals ${s}`);
    if (!g || !g.next_run_at.startsWith('2099')) throw new Error(JSON.stringify(g));
    cleanup.push(`gen:${g.id}`);
  });

  await step('the email designer', async () => {
    campId = (await admin.post('/marketing/campaigns', { name: `Design ${s}`, subject: 'Hi' })).data.id;
    await pa.goto(B + `/marketing/campaigns/${campId}`);
    await pa.click('[data-message-kind="designed"]');
    await pa.locator('[data-email-designer]').waitFor();
    await pa.click('[data-palette="divider"]');
    await pa.locator('[data-block="divider"]').waitFor();
    await pa.locator('[data-block="heading"] button').first().click();
    await pa.fill('#d-h', `Spring at ${s}`);
    await pa.frameLocator('[data-design-preview]').locator(`text=Spring at ${s}`).waitFor({ timeout: 10000 });
    await pa.click('button:has-text("Save")');
    await pa.locator('button:has-text("Saved")').waitFor();
    const c = (await admin.get(`/marketing/campaigns/${campId}`)).data;
    if (!c.design || c.design.blocks.length !== 4 || c.design.blocks[0].text !== `Spring at ${s}`) throw new Error(JSON.stringify(c.design));
  });

  await step('no browser errors', async () => { if (errors.length) throw new Error(errors.join(' | ')); });
} finally {
  await browser.close();
  for (const c of cleanup) {
    if (c.startsWith('link:')) await admin.del(`/tracking/links/${c.slice(5)}`);
    else if (c.startsWith('gen:')) await admin.del(`/marketing/generators/${c.slice(4)}`);
    else { await admin.del(`/crm/contact/items/${c}`); await admin.del(`/crm/lead/items/${c}`); }
  }
  for (const x of (await admin.get(`/crm/lead/items?q=walker.${s}`)).data.items ?? []) await admin.del(`/crm/lead/items/${x.id}`);
  if (campId) await admin.del(`/marketing/campaigns/${campId}`);
  if (segId) await admin.del(`/marketing/segments/${segId}`);
  if (formId) await admin.del(`/crm/webforms/${formId}`);
  if (siteId) await admin.del(`/tracking/sites/${siteId}`);
  await cleanupRun(s);
}
console.log(failed ? `\n${failed} failed` : '\nall passed');
process.exit(failed ? 1 : 0);
