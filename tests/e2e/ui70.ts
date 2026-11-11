// CRM depth in the browser: who sees a custom field, automation test mode, the duplicate
// search (merging a group), gross profit in analytics, importing a vCard file, and a record's
// booking link.
import { chromium } from 'playwright';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
const B = 'http://localhost:8080';
const s = Date.now().toString(36);
const digits = String(Date.now()).slice(-7);
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: string[] = [];
let failed = 0, n = 0;
const pages: import('playwright').Page[] = [];
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `fail70-${n}-${i}.png`, fullPage: true }).catch(() => {}); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const fields: string[] = [], items: string[] = [];
let catId = '';

const signIn = async (email: string, password: string) => {
  const ctx = await browser.newContext({ viewport: { width: 1400, height: 950 }, timezoneId: 'UTC', locale: 'en-GB' });
  const p = await ctx.newPage();
  p.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
  p.on('console', (m) => m.type() === 'error' && !/400|401|403|404|409|Failed to load resource/.test(m.text()) && errors.push(m.text()));
  pages.push(p);
  await p.goto(B + '/login'); await p.fill('#email', email); await p.fill('#password', password);
  await p.click('button:has-text("Sign in")'); await p.waitForURL(B + '/');
  return p;
};

try {
  const bob = (await admin.post('/users', { email: `bob.${s}@acme.test`, password: 'password123', first_name: 'Bob', last_name: `Ui${s}` })).data;
  const field = (await admin.post('/crm/deal/fields', { label: `Margin ${s}`, kind: 'number' })).data;
  fields.push(field.id);
  const pipe = (await admin.post('/crm/deal/categories', { name: `Ui pipe ${s}` })).data.categories.find((c: any) => c.name === `Ui pipe ${s}`);
  catId = pipe.id;
  const base = (await admin.get('/crm/config')).data.base_currency;
  const product = (await admin.post('/crm/products', { name: `Ui widget ${s}`, price: '100', cost: '30', currency: base })).data;
  const deal = (await admin.post('/crm/deal/items', { title: `Ui deal ${s}`, category_id: pipe.id, currency: base })).data;
  items.push(deal.id);
  await admin.put(`/crm/items/${deal.id}/products`, { rows: [{ product_id: product.id, name: product.name, price: '100', quantity: '2' }] });
  await admin.patch(`/crm/deal/items/${deal.id}`, { stage_id: pipe.stages.find((x: any) => x.semantics === 'success').id });
  for (const first of ['Ann', 'Anna']) items.push((await admin.post('/crm/contact/items', { first_name: first, last_name: `Dup${s}`, multifields: [{ kind: 'phone', value: `+1 555 ${digits}` }] })).data.id);
  await admin.post('/calendar/slot-links', { title: `Ui demo ${s}`, hours: [1, 2, 3, 4, 5, 6, 7].map((d) => [d, 0, 1440]), timezone: 'UTC' });

  const pa = await signIn('admin@acme.test', 'supersecret1');

  await step('choosing who sees a custom field', async () => {
    await pa.goto(B + '/crm/settings?tab=fields');
    await pa.click(`[data-field-audience="${field.code}"]`);
    const dlg = pa.locator('dialog[open]');
    await dlg.locator('input[placeholder="Everyone"]').fill('Bob');
    await pa.locator(`text=Bob Ui${s}`).first().click();
    await dlg.locator('button:has-text("Save")').click();
    await pa.locator(`[data-field-audience="${field.code}"]:has-text("Seen by 1 chosen")`).waitFor();
    const f = (await admin.get('/crm/deal/fields')).data.find((x: any) => x.id === field.id);
    if (!f.visible_user_ids.includes(bob.id)) throw new Error(JSON.stringify(f));
  });

  await step('automation test mode', async () => {
    await pa.goto(B + '/crm/settings?tab=automation');
    await pa.locator('select[aria-label="Pipeline"]').selectOption(pipe.id);
    const bar = pa.locator('[data-test-mode]');
    await bar.locator('input[type="checkbox"]').check();
    await bar.locator('input[placeholder="Testers"]').fill('Bob');
    await pa.locator(`text=Bob Ui${s}`).first().click();
    await bar.locator('button:has-text("Save")').click();
    await pa.locator('text=Test mode on').first().waitFor();
    const ov = (await admin.get(`/crm/automation?category_id=${pipe.id}`)).data.test_mode;
    if (!ov.enabled || !ov.user_ids.includes(bob.id)) throw new Error(JSON.stringify(ov));
  });

  await step('the duplicate search merges a group', async () => {
    await pa.goto(B + '/crm/contact');
    await pa.click('[data-find-duplicates]');
    await pa.waitForURL(/\/crm\/contact\/duplicates/);
    await pa.fill('input[aria-label="Search duplicates"]', digits);
    const g = pa.locator('[data-dup-group="phone"]', { hasText: digits });
    await g.waitFor();
    pa.once('dialog', (d) => d.accept());
    await g.locator('[data-merge-group]').click();
    await pa.locator('[data-no-duplicates]').waitFor();
    const left = (await admin.get(`/crm/contact/items?q=dup${s}`)).data.items;
    if (left.length !== 1) throw new Error(`${left.length} left`);
  });

  await step('gross profit in analytics', async () => {
    await pa.goto(B + `/crm/analytics?pipeline=${pipe.id}`);
    const gp = pa.locator('[data-gross-profit-report]');
    await gp.locator('[data-total-margin]:has-text("70.0%")').waitFor();
    await gp.locator('button[role="tab"]:has-text("Products")').click();
    await gp.locator(`text=Ui widget ${s}`).waitFor();
  });

  await step('importing a vCard file', async () => {
    await pa.goto(B + '/crm/contact/import');
    const vcf = `BEGIN:VCARD\r\nVERSION:3.0\r\nN:Card${s};Vera\r\nFN:Vera Card${s}\r\nEMAIL:vera.${s}@cards.example\r\nEND:VCARD\r\n`;
    await pa.locator('input[type="file"]').setInputFiles({ name: `people-${s}.vcf`, mimeType: 'text/vcard', buffer: Buffer.from(vcf) });
    await pa.locator('[data-vcard-preview]:has-text("1 card")').waitFor();
    await pa.click('button:has-text("Start import")');
    await pa.locator('text=Import finished').waitFor({ timeout: 20000 });
    const v = (await admin.get(`/crm/contact/items?q=card${s}`)).data.items;
    if (v.length !== 1) throw new Error(JSON.stringify(v));
    items.push(v[0].id);
  });

  await step('a record\'s booking link', async () => {
    await pa.goto(B + `/crm/deal/${deal.id}`);
    await pa.click('[data-booking-link]');
    await pa.locator(`[role="dialog"] button:has-text("Ui demo ${s}")`).click();
    const url = await pa.locator('[data-booking-url]').inputValue();
    if (!/\/meet\/.+\?r=/.test(url)) throw new Error(url);
  });

  await step('no browser errors', async () => { if (errors.length) throw new Error(errors.join(' | ')); });
} finally {
  await browser.close();
  for (const id of fields) await admin.del(`/crm/deal/fields/${id}`);
  for (const id of items) await admin.del(`/crm/${id === items[0] ? 'deal' : 'contact'}/items/${id}`);
  if (catId) await admin.put('/crm/automation/test-mode', { category_id: catId, enabled: false });
  await cleanupRun(s);
}
console.log(failed ? `\n${failed} failed` : '\nall passed');
process.exit(failed ? 1 : 0);
