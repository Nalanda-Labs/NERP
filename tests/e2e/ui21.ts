// Marketing in the app: build a segment, write and send a campaign (with a test send), read
// its results, unsubscribe through the public page, manage the unsubscribe list.
import { chromium } from 'playwright';
import { Client, waitFor } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
const B = 'http://localhost:8080';
const MP = 'http://localhost:8026/api/v1';
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: string[] = [];
const page = await (await browser.newContext({ viewport: { width: 1440, height: 1000 } })).newPage();
page.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
page.on('console', (m) => m.type() === 'error' && !/401|400|404|Failed to load resource/.test(m.text()) && errors.push(m.text()));
const s = Date.now().toString(36);
const answers: (string | boolean)[] = [];
page.on('dialog', (d) => { const a = answers.shift(); a === false ? d.dismiss() : d.accept(typeof a === 'string' ? a : undefined); });
let n = 0, failed = 0;
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); await page.screenshot({ path: `fail21-${++n}.png`, fullPage: true }); }
};
const mailpit = async (q: string): Promise<any[]> => (await (await fetch(`${MP}/search?query=${encodeURIComponent(q)}`)).json()).messages ?? [];

const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const mk = async (first: string) => (await admin.post('/crm/contact/items', { first_name: first, last_name: `Promo${s}`, multifields: [{ kind: 'email', value_type: 'work', value: `${first.toLowerCase()}.${s}@example.com` }] })).data;
const una = await mk('Una');
await mk('Vic');

await page.goto(B + '/login'); await page.fill('#email', 'admin@acme.test'); await page.fill('#password', 'supersecret1');
await page.click('button:has-text("Sign in")'); await page.waitForURL(B + '/');
let campaignUrl = '';

await step('segment with a live count', async () => {
  await page.click('aside a:has-text("Segments")');
  await page.click('button:has-text("New segment")');
  await page.fill('#sg-name', `Promo people ${s}`);
  await page.fill('#sg-q', `Promo${s}`);
  await page.locator('[data-reach]:text-is("2")').waitFor();
  await page.click('dialog[open] button:has-text("Save")');
  await page.waitForSelector('text=Segment saved');
  await page.locator(`[data-segment]:has-text("Promo people ${s}") >> text=2`).first().waitFor();
});

await step('write a campaign', async () => {
  await page.click('aside a:has-text("Campaigns")');
  answers.push(`Launch ${s}`);
  await page.click('button:has-text("New campaign")');
  await page.waitForURL(/\/marketing\/campaigns\/[0-9a-f-]{36}/);
  campaignUrl = page.url();
  await page.fill('#cp-from', 'Acme team');
  await page.fill('#cp-subject', `Launch day ${s}`);
  await page.fill('#cp-body', 'Hello ,\n\nhttps://example.com/launch\n');
  await page.locator('#cp-body').evaluate((el: HTMLTextAreaElement) => el.setSelectionRange(6, 6));
  await page.click('button:has-text("{{first_name}}")');
  await page.check(`label:has-text("Promo people ${s}") input`);
  await page.locator('[data-reach]:text-is("2")').waitFor();
  await page.click('button:has-text("Save")');
  await page.waitForSelector('text=Saved');
});

await step('send a test', async () => {
  await page.fill('input[aria-label="Test address"]', `check.${s}@example.com`);
  await page.click('section[aria-labelledby="test-h"] button:has-text("Send")');
  await page.waitForSelector(`text=Test sent to check.${s}@example.com`);
  const m = await waitFor(async () => (await mailpit(`to:check.${s}@example.com`))[0], 15000);
  if (!m?.Subject.startsWith('[Test] Launch day')) throw new Error('no test email');
});

await step('send now, then results', async () => {
  answers.push(true);
  await page.click('button:has-text("Send now")');
  await page.waitForSelector('text=Sending started');
  await page.locator('[data-status="sent"]').waitFor({ timeout: 30000 });
  await page.locator('[data-tile="Sent"] >> text=2').waitFor();
  const m = await waitFor(async () => (await mailpit(`to:una.${s}@example.com subject:"Launch day"`))[0], 15000);
  const html: string = (await (await fetch(`${MP}/message/${m.ID}`)).json()).HTML;
  if (!html.startsWith('<div') || !html.includes('Hello Una,')) throw new Error('not filled in: ' + html.slice(0, 120));
  const pixel = html.match(/src="([^"]*\/api\/public\/m\/o\/[^"]+)"/)![1].replace(/^https?:\/\/[^/]+/, B);
  await fetch(pixel);
  campaignUrl && (await page.reload());
  await page.locator('[data-tile="Opened"] >> text=1').waitFor();
  await page.locator('[data-tile="Opened"] >> text=50%').waitFor();
  await page.selectOption('select[aria-label="Show"]', 'opened');
  await page.locator(`[data-recipient="vic.${s}@example.com"]`).waitFor({ state: 'detached' });
  await page.locator(`[data-recipient="una.${s}@example.com"]`).waitFor();
  const unsub = html.match(/href="([^"]*\/unsubscribe\/[^"]+)"/)![1].replace(/^https?:\/\/[^/]+/, B);
  (globalThis as any).unsub = unsub;
});

await step('unsubscribe through the public page', async () => {
  const pub = await browser.newPage();
  await pub.goto((globalThis as any).unsub);
  await pub.locator(`text=u***@example.com will stop getting marketing emails`).waitFor();
  await pub.click('button:has-text("Unsubscribe")');
  await pub.locator('h1:text-is("You\'re unsubscribed")').waitFor();
  await pub.close();
});

await step('the unsubscribe list', async () => {
  await page.click('aside a:has-text("Unsubscribes")');
  const row = page.locator(`[data-optout="una.${s}@example.com"]`);
  await row.locator(`text=Unsubscribed from “Launch ${s}”`).waitFor();
  answers.push(true);
  await row.locator('button:has-text("Remove")').click();
  await row.waitFor({ state: 'detached' });
});

await step('the record shows the campaign', async () => {
  await page.goto(`${B}/crm/contact/${una.id}`);
  await page.locator(`text=sent the campaign “Launch ${s}”`).waitFor();
  await page.locator(`text=unsubscribed from marketing emails`).waitFor();
});

await page.goto(campaignUrl);
await page.locator('[data-tile="Sent"]').waitFor().catch(() => {});
await page.screenshot({ path: '39-campaign.png', fullPage: true });
await cleanupRun(s);
console.log(errors.length ? 'console errors:\n' + errors.join('\n') : 'no console errors');
await browser.close();
process.exit(failed || errors.length ? 1 : 0);
