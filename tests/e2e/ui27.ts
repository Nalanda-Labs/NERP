// E-signature in the browser: send a deal's document for signature, sign it on the public
// page (drawn signature), see it signed on the deal.
import { chromium } from 'playwright';
import { Client, waitFor } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
const B = 'http://localhost:8080';
const MP = 'http://localhost:8026/api/v1';
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: string[] = [];
const watch = (p: import('playwright').Page) => {
  p.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
  p.on('console', (m) => m.type() === 'error' && !/401|400|404|Failed to load resource/.test(m.text()) && errors.push(m.text()));
  p.on('dialog', (d) => d.accept());
};
const page = await (await browser.newContext({ viewport: { width: 1440, height: 1000 } })).newPage();
watch(page);
const s = Date.now().toString(36);
let n = 0, failed = 0;
const pages = [page];
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `fail27-${n}-${i}.png`, fullPage: true }); }
};

const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const deal = (await admin.post('/crm/deal/items', { title: `Lease ${s}` })).data;
const tpl = (await admin.get('/crm/doc-templates?type=deal')).data.items.find((t: any) => t.builtin_key === 'typst.invoice');
const doc = (await admin.post(`/crm/items/${deal.id}/documents`, { template_id: tpl.id })).data;

await page.goto(B + '/login'); await page.fill('#email', 'admin@acme.test'); await page.fill('#password', 'supersecret1');
await page.click('button:has-text("Sign in")'); await page.waitForURL(B + '/');

try {
  await step('send for signature', async () => {
    await page.goto(`${B}/crm/deal/${deal.id}`);
    await page.click('button:has-text("Send for signature")');
    await page.fill('input[aria-label="Signer 1 name"]', `Rae Long ${s}`);
    await page.fill('input[aria-label="Signer 1 email"]', `rae.${s}@example.com`);
    await page.click('dialog[open] button:has-text("Send")');
    await page.locator('[data-sign-status="sent"]').filter({ hasText: '0 of 1 signed' }).waitFor();
  });

  await step('sign on the public page', async () => {
    const m = await waitFor(async () => (await (await fetch(`${MP}/search?query=${encodeURIComponent(`to:rae.${s}@example.com subject:"Please sign ${doc.number}"`)}`)).json()).messages?.[0], 15000);
    const text: string = (await (await fetch(`${MP}/message/${m.ID}`)).json()).Text;
    const signer = await (await browser.newContext({ viewport: { width: 1000, height: 1200 } })).newPage();
    watch(signer);
    pages.push(signer);
    await signer.goto(`${B}/sign/${text.match(/\/sign\/([\w-]+)/)![1]}`);
    await signer.locator(`h2:text-is("Sign as Rae Long ${s}")`).waitFor();
    await signer.locator('canvas').scrollIntoViewIfNeeded();
    const box = (await signer.locator('canvas').boundingBox())!;
    await signer.mouse.move(box.x + 30, box.y + box.height / 2);
    await signer.mouse.down();
    for (let i = 1; i <= 12; i++) await signer.mouse.move(box.x + 30 + i * 25, box.y + box.height / 2 + (i % 2 ? -18 : 18));
    await signer.mouse.up();
    await signer.check('label:has-text("sign it electronically") input');
    await signer.click('button:text-is("Sign")');
    await signer.locator('[data-done] >> text=Everyone has signed.').waitFor();
    await signer.screenshot({ path: '47-signed.png' });
  });

  await step('signed on the deal', async () => {
    await page.reload();
    await page.locator('[data-sign-status="completed"]').click();
    await page.locator('[data-sign-request] >> text=Download the signed copy').waitFor();
    await page.locator(`text=${doc.number} is signed by everyone`).waitFor();
  });
} finally {
  await cleanupRun(s);
}
console.log(errors.length ? 'console errors:\n' + errors.join('\n') : 'no console errors');
await browser.close();
process.exit(failed || errors.length ? 1 : 0);
