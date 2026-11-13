// Text messages in the browser (Twilio stand-in on :9466): System → Text messages, texting a
// client from a CRM record and seeing the conversation, and an SMS campaign's editor.
import { chromium } from 'playwright';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
import { OUTPUT } from './output.ts';
import { dbUrl } from '../db/cleanup.ts';
import { startMockTwilio, waitFor } from './mock-twilio.ts';
import pg from 'pg';
const B = 'http://localhost:8080';
const s = Date.now().toString(36);
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: string[] = [];
let failed = 0, n = 0;
const pages: import('playwright').Page[] = [];
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `${OUTPUT}/fail96-${n}-${i}.png`, fullPage: true }).catch(() => {}); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const db = new pg.Client({ connectionString: dbUrl });
await db.connect();
const tw = await startMockTwilio();
const before = (await admin.get('/sms/setup')).data;
const phone = `+4420794${String(Number.parseInt(s.slice(-4), 36) % 90000 + 10000)}`;
const ann = (await admin.post('/crm/contact/items', { first_name: 'Ann', last_name: `Ui${s}`, multifields: [{ kind: 'phone', value: phone }] })).data;
let campaignId = '';

const ctx = await browser.newContext({ viewport: { width: 1400, height: 950 }, timezoneId: 'UTC', locale: 'en-GB' });
const pa = await ctx.newPage();
pa.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
pa.on('console', (m) => m.type() === 'error' && !/400|401|403|404|Failed to load resource/i.test(m.text()) && errors.push(m.text()));
pages.push(pa);
await pa.goto(B + '/login'); await pa.fill('#email', 'admin@acme.test'); await pa.fill('#password', 'supersecret1');
await pa.click('button:has-text("Sign in")'); await pa.waitForURL(B + '/');

try {
  await step('System → Text messages: set-up and the address for Twilio', async () => {
    await pa.goto(`${B}/system/sms`);
    await pa.locator('[data-sms-enabled="true"]').waitFor();
    await pa.locator('[data-inbound-url]:has-text("/api/public/twilio/sms/inbound")').waitFor();
    await pa.fill('#sms-cc', '+44');
    await pa.click('button:has-text("Save")');
    await pa.locator('text=Saved').first().waitFor();
    if ((await admin.get('/sms/setup')).data.country_code !== '+44') throw new Error('not saved');
  });
  await step('texting the client from the record', async () => {
    await pa.goto(`${B}/crm/contact/${ann.id}`);
    await pa.click('button:has-text("Text")');
    await pa.locator('[data-sms-composer]').waitFor();
    await pa.fill('[data-sms-body]', `Hi Ann ${s}`);
    await pa.locator('[data-sms-count]:has-text("characters")').waitFor();
    await pa.click('[data-sms-send]');
    await waitFor(tw.sent, 1);
    if (!tw.sent.some((x) => x.to === phone && x.body === `Hi Ann ${s}`)) throw new Error(JSON.stringify(tw.sent));
    await pa.locator(`[data-sms-thread] li:has-text("Hi Ann ${s}")`).waitFor();
  });
  await step('…and it shows in the message log', async () => {
    await pa.goto(`${B}/system/sms`);
    await pa.locator(`[data-sms-log] tr:has-text("Hi Ann ${s}")`).waitFor();
  });
  await step('an SMS campaign\'s editor', async () => {
    campaignId = (await admin.post('/marketing/campaigns', { name: `Texts ${s}` })).data.id;
    await pa.goto(`${B}/marketing/campaigns/${campaignId}`);
    await pa.click('[data-channel="sms"]');
    await pa.locator('[data-sms-campaign-body]').waitFor();
    if (await pa.locator('#cp-subject').count()) throw new Error('subject shown for a text');
    await pa.fill('[data-sms-campaign-body]', `Sale ${s}`);
    await pa.click('button:has-text("Save")');
    await pa.locator('button:has-text("Saved")').waitFor();
    const c = (await admin.get(`/marketing/campaigns/${campaignId}`)).data;
    if (c.channel !== 'sms' || c.body !== `Sale ${s}`) throw new Error(JSON.stringify(c));
  });
  await step('no browser errors', async () => { if (errors.length) throw new Error(errors.join(' | ')); });
} finally {
  await browser.close();
  await admin.put('/sms/setup', { from: before.from ?? '', country_code: before.country_code ?? '' }).catch(() => {});
  if (campaignId) await db.query('DELETE FROM mkt_campaigns WHERE id = $1', [campaignId]).catch(() => {});
  await db.query('DELETE FROM sms_messages WHERE body LIKE $1', [`%${s}%`]).catch(() => {});
  await db.end();
  await tw.close();
  await cleanupRun(s);
}
console.log(failed ? `\n${failed} failed` : '\nall passed');
process.exit(failed ? 1 : 0);
