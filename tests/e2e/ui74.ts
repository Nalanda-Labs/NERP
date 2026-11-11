// Step 3 in the browser: moving over from another CRM (a Pipedrive export: recognised, a
// column changed, the pipeline, stages and owner checked, brought over, the result shown,
// undone), and the System pages: point-in-time restore on Backups, encryption at rest on
// Storage.
import { chromium } from 'playwright';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { dbUrl } from '../db/cleanup.ts';
import { writeFileSync } from 'node:fs';
import { OUTPUT } from './output.ts';
import pg from 'pg';
const B = 'http://localhost:8080';
const s = Date.now().toString(36);
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: string[] = [];
let failed = 0, n = 0;
const pages: import('playwright').Page[] = [];
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `${OUTPUT}/fail74-${n}-${i}.png`, fullPage: true }).catch(() => {}); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();

const ctx = await browser.newContext({ viewport: { width: 1400, height: 950 }, timezoneId: 'UTC', locale: 'en-GB' });
const pa = await ctx.newPage();
pa.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
pa.on('console', (m) => m.type() === 'error' && !/400|401|403|404|409|Failed to load resource/.test(m.text()) && errors.push(m.text() + ' @ ' + pa.url()));
pages.push(pa);
await pa.goto(B + '/login'); await pa.fill('#email', 'admin@acme.test'); await pa.fill('#password', 'supersecret1');
await pa.click('button:has-text("Sign in")'); await pa.waitForURL(B + '/');

// A Pipedrive export: organisations, people and deals (one deal's notes column left unmapped).
const orgs = `Organization - ID,Organization - Name,Organization - Address,Organization - Owner\r\n1,Umbrella ${s},Main St 1,admin@acme.test\r\n`;
const people = `Person - ID,Person - Name,Person - Email,Person - Phone,Person - Organization,Person - Owner\r\n7,Rita Pd${s},rita.${s}@umbrella.test,+1 555 0300,Umbrella ${s},admin@acme.test\r\n`;
const deals = `Deal - ID,Deal - Title,Deal - Value,Deal - Currency,Deal - Pipeline,Deal - Stage,Deal - Organization,Deal - Contact person,Deal - Owner,Deal - Secret notes\r\n42,Umbrella deal ${s},900,EUR,Field ${s},Proposal made,Umbrella ${s},Rita Pd${s},admin@acme.test,do not import\r\n`;
const files = ['organizations', 'persons', 'deals'].map((n) => `${OUTPUT}/${n}-${s}.csv`);
writeFileSync(files[0], orgs); writeFileSync(files[1], people); writeFileSync(files[2], deals);
let migrationId = '';

try {
  await step('the page: where each system\'s export comes from', async () => {
    await pa.goto(B + '/crm/migrate');
    await pa.locator('h1:has-text("Move from another CRM")').waitFor();
    await pa.click('label:has-text("Pipedrive")');
    await pa.locator('text=Settings → Export data').waitFor();
  });

  await step('the export is read and recognised', async () => {
    await pa.setInputFiles('[data-migrate-files]', files);
    await pa.locator('h2:has-text("What is in it (Pipedrive)")').waitFor({ timeout: 20000 });
    const kind = async (f: string) => pa.locator(`[data-file="${f}"] [data-kind]`).inputValue();
    if (await kind(`organizations-${s}.csv`) !== 'company' || await kind(`persons-${s}.csv`) !== 'contact' || await kind(`deals-${s}.csv`) !== 'deal') throw new Error('kinds not recognised');
    const dealFile = pa.locator(`[data-file="deals-${s}.csv"]`);
    if (!(await dealFile.evaluate((d) => (d as HTMLDetailsElement).open))) await dealFile.locator('summary').click();
    const sel = (c: string) => dealFile.locator(`[data-column="${c}"] select`);
    if (await sel('Deal - Value').inputValue() !== 'amount' || await sel('Deal - Contact person').inputValue() !== 'contact_name') throw new Error('columns not mapped');
    // A column the person maps by hand: the secret notes stay out, the title is chosen again.
    if (await sel('Deal - Secret notes').inputValue() !== '') throw new Error('an unknown column was mapped');
    await sel('Deal - Secret notes').selectOption('description');
    await sel('Deal - Secret notes').selectOption('');
  });

  await step('pipeline, stage and owner to check', async () => {
    await pa.locator(`[data-pipeline="Field ${s}"] select`).waitFor();
    if (await pa.locator(`[data-pipeline="Field ${s}"] select`).inputValue() !== 'new') throw new Error('a new pipeline should be offered');
    await pa.locator('[data-stage="Proposal made"]').waitFor();
    const me = await pa.locator('[data-owner="admin@acme.test"] select').inputValue();
    if (!/^[0-9a-f-]{36}$/.test(me)) throw new Error('the owner was not matched by email');
    await pa.locator('[data-currencies]').count(); // EUR may or may not be enabled here
  });

  await step('brought over, with the result shown', async () => {
    await pa.click('[data-migrate-run]');
    await pa.locator('[data-migration="done"]').waitFor({ timeout: 60000 });
    for (const k of ['company', 'contact', 'deal']) {
      const created = await pa.locator(`[data-count="${k}"] td`).nth(1).innerText();
      if (created !== '1') throw new Error(`${k}: ${created} created`);
    }
    const deal = (await admin.get(`/crm/deal/items?q=${encodeURIComponent(`Umbrella deal ${s}`)}`)).data.items[0];
    const d = (await admin.get(`/crm/deal/items/${deal.id}`)).data;
    if (d.company_title !== `Umbrella ${s}` || d.contacts.length !== 1 || Number(d.opportunity) !== 900 || (d.comments ?? '').includes('do not import')) throw new Error(JSON.stringify(d).slice(0, 400));
    migrationId = (await admin.get('/crm/migrations')).data.items[0].id;
    await pa.locator(`[data-history="${migrationId}"]`).waitFor();
  });

  await step('undone from the page', async () => {
    pa.once('dialog', (d) => d.accept());
    await pa.click('[data-migration="done"] button:has-text("Undo")');
    await pa.locator('[data-migration="undone"]').waitFor({ timeout: 15000 });
    if ((await admin.get(`/crm/deal/items?q=${encodeURIComponent(`Umbrella deal ${s}`)}`)).data.items.length) throw new Error('the deal is still there');
  });

  await step('Backups: point-in-time restore', async () => {
    await pa.goto(B + '/system/backups');
    const sec = pa.locator('[data-pitr]');
    await sec.waitFor();
    await sec.locator('[data-receiving]').waitFor();
    if (!(await sec.locator('[data-pitr-command]').innerText()).includes('nerp-pitr-restore.sh')) throw new Error('no command');
  });

  await step('Storage: encryption at rest', async () => {
    await pa.goto(B + '/system/storage');
    const sec = pa.locator('section[aria-label="Encryption"]');
    await sec.waitFor();
    const on = await sec.locator('[data-encryption]').getAttribute('data-encryption');
    const api = (await admin.get('/admin/storage')).data.encryption;
    if ((on === 'on') !== api.enabled) throw new Error(`page says ${on}`);
    if (api.enabled && !(await sec.innerText()).includes(api.key_id)) throw new Error('the key id is not shown');
  });

  await step('no browser errors', async () => { if (errors.length) throw new Error(errors.join(' | ')); });
} finally {
  await browser.close();
  await db.query(`DELETE FROM crm_items WHERE title ILIKE $1 OR last_name ILIKE $1 OR company_name ILIKE $1`, [`%${s}%`]).catch(() => {});
  await db.query('DELETE FROM crm_categories WHERE name = $1', [`Field ${s}`]).catch(() => {});
  if (migrationId) await db.query('DELETE FROM crm_migrations WHERE id = $1', [migrationId]).catch(() => {});
  await db.end();
  await cleanupRun(s);
}
console.log(failed ? `\n${failed} failed` : '\nall passed');
process.exit(failed ? 1 : 0);
