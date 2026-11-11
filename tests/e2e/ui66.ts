// Ready-made things in the browser: a business process from the lists gallery, a web form
// from a template, a stage's required fields set in automation and enforced on the record,
// a segment imported from pasted addresses, and a campaign message started from a template.
import { chromium } from 'playwright';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
const B = 'http://localhost:8080';
const s = Date.now().toString(36);
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: string[] = [];
let failed = 0, n = 0;
let p!: import('playwright').Page;
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); await p?.screenshot({ path: `fail66-${++n}.png`, fullPage: true }); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const cfg = (await admin.get('/crm/config')).data;
const cat = cfg.types.find((t: any) => t.code === 'deal').categories.find((c: any) => c.is_default);
const stage = cat.stages.filter((x: any) => x.semantics === 'process')[1];
const stageBefore = stage.required_fields ?? [];
const known = `known.${s}@example.com`;
await admin.post('/crm/contact/items', { first_name: 'Known', last_name: `Ui${s}`, multifields: [{ kind: 'email', value: known }] });
const camp = (await admin.post('/marketing/campaigns', { name: `Ui camp ${s}` })).data;

try {
  p = await (await browser.newContext({ viewport: { width: 1280, height: 900 }, timezoneId: 'UTC', locale: 'en-GB' })).newPage();
  p.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
  p.on('console', (m) => m.type() === 'error' && !/400|401|403|404|409|Failed to load resource/.test(m.text()) && errors.push(m.text()));
  await p.goto(B + '/login'); await p.fill('#email', 'admin@acme.test'); await p.fill('#password', 'supersecret1');
  await p.click('button:has-text("Sign in")'); await p.waitForURL(B + '/');

  await step('a business process from the gallery', async () => {
    await p.goto(B + '/lists');
    await p.click('button:has-text("Ready-made")');
    await p.click('[data-solution="trips"]');
    await p.click('dialog[open] button:has-text("Create")');
    await p.waitForURL(/\/lists\/[0-9a-f-]{36}/);
    const id = p.url().split('/lists/')[1].split(/[?#/]/)[0];
    await admin.patch(`/lists/${id}`, { name: `Trips ${s}` });
    const wf = (await admin.get(`/lists/${id}/workflows`)).data;
    if (!(wf.items ?? wf).length) throw new Error('no approval process');
  });

  await step('a web form from a template', async () => {
    await p.goto(B + '/crm/webforms');
    await p.click('button:has-text("New form")');
    await p.selectOption('#wf-tpl', 'quote');
    const name = p.locator('#wf-name');
    if ((await name.inputValue()) !== 'Quote request') throw new Error('name: ' + (await name.inputValue()));
    await name.fill(`Quote ${s}`);
    await p.click('button:has-text("Create form")');
    await p.waitForTimeout(800);
    const row = (await admin.get('/crm/webforms')).data.items.find((x: any) => x.name === `Quote ${s}`);
    if (!row) throw new Error('not saved');
    const f = (await admin.get(`/crm/webforms/${row.id}`)).data;
    if (f.fields?.length !== 6) throw new Error(JSON.stringify(f).slice(0, 300));
  });

  await step('a stage asks for fields', async () => {
    await p.goto(B + '/crm/settings?tab=automation');
    const col = p.locator(`section[aria-label="Stage ${stage.name}"]`);
    await col.locator('[data-required-fields]').click();
    await p.locator('dialog[open] label:has-text("Amount") input').check();
    await p.click('dialog[open] button:has-text("Save")');
    await col.locator('[data-required-fields]:has-text("Requires 1 field")').waitFor();
    const deal = (await admin.post('/crm/deal/items', { title: `Ui deal ${s}`, category_id: cat.id, stage_id: cat.stages[0].id, currency: cfg.base_currency })).data;
    const r = await admin.patch(`/crm/deal/items/${deal.id}`, { stage_id: stage.id });
    if (r.status !== 400) throw new Error('moved without the amount');
  });

  await step('a segment from pasted addresses', async () => {
    await p.goto(B + '/marketing/segments');
    await p.click('button:has-text("Import")');
    await p.fill('#im-name', `Ui fair ${s}`);
    await p.fill('#im-text', `email,first name,last name\n${known},Known,Ui${s}\nfresh.${s}@example.com,Fresh,Ui${s}`);
    await p.locator('dialog[open] :text("2 rows")').waitFor();
    await p.click('dialog[open] button:has-text("Import")');
    const res = p.locator('[data-import-result]');
    await res.waitFor();
    const t = await res.innerText();
    if (!/1 already in the CRM/.test(t) || !/1 added as new contacts/.test(t)) throw new Error(t);
    await p.click('button:has-text("Done")');
    await p.locator(`button:has-text("Ui fair ${s}") >> text=imported list`).waitFor();
  });

  await step('a campaign started from a template', async () => {
    await p.goto(B + `/marketing/campaigns/${camp.id}`);
    await p.selectOption('[data-start-from]', { label: 'Event invitation' });
    const subj = await p.inputValue('#cp-subject');
    if (!subj.startsWith('You are invited')) throw new Error(subj);
    if (!(await p.inputValue('#cp-body')).includes('{{first_name}}')) throw new Error('body');
  });

  await step('no browser errors', async () => { if (errors.length) throw new Error(errors.join(' | ')); });
} finally {
  await browser.close();
  await admin.patch(`/crm/stages/${stage.id}`, { required_fields: stageBefore });
  await cleanupRun(s);
}
console.log(failed ? `\n${failed} failed` : '\nall passed');
process.exit(failed ? 1 : 0);
