// Web forms UI: build a form, hosted page, embed on another site with UTM tags.
import { chromium } from 'playwright';
const B = 'http://localhost:8080';
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: any[] = [];
const ctx = await browser.newContext({ viewport: { width: 1440, height: 1000 } });
const page = await ctx.newPage();
page.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
page.on('console', (m) => m.type() === 'error' && !/401|400|Failed to load resource/.test(m.text()) && errors.push(m.text()));
let n = 0, failed = 0;
const step = async (name, fn) => { try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); await page.screenshot({ path: `fail12-${++n}.png`, fullPage: true }); } };
const s = Date.now().toString(36);

import http from 'node:http';
import { cleanupRun } from './cleanup.ts';
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const api = async (method: string, path: string, body?: any, token?: string | null) => (await fetch(B + '/api' + path, { method, headers: { 'content-type': 'application/json', ...(token ? { authorization: `Bearer ${token}` } : {}) }, body: body ? JSON.stringify(body) : undefined })).json();
const token = (await api('POST', '/auth/login', { email: 'admin@acme.test', password: 'supersecret1' })).access_token;
let key, formId;

await page.goto(B + '/login'); await page.fill('#email', 'admin@acme.test'); await page.fill('#password', 'supersecret1');
await page.click('button:has-text("Sign in")'); await page.waitForURL(B + '/');

await step('build a form with live preview', async () => {
  await page.click('aside >> text=Web forms');
  await page.click('button:has-text("New form")');
  await page.fill('#wf-name', `Demo request ${s}`);
  await page.fill('#wf-title', 'Book a demo');
  await page.click('button:has-text("Add field")');
  const last = page.locator('section:has(h2:text-is("Fields")) li').last();
  await last.locator('input[aria-label="Label"]').fill('Company size');
  await last.locator('select[aria-label="Type"]').selectOption('select');
  await last.locator('textarea[aria-label="Options"]').fill('1-10\n11-100\n100+');
  await page.locator('div:has(> p:text-is("Preview")) >> text=Book a demo').waitFor();
  await page.locator('div:has(> p:text-is("Preview")) >> label:has-text("Company size")').waitFor();
  await page.screenshot({ path: '24-webform-builder.png', fullPage: true });
  await page.click('button:has-text("Create form")');
  await page.waitForSelector('text=Form saved');
  const list = await api('GET', '/crm/webforms', null, token);
  const f = list.items.find((x) => x.name === `Demo request ${s}`);
  key = f.key; formId = f.id;
});

await step('share tab shows link and embed code', async () => {
  await page.click('nav >> button:has-text("Share & embed")');
  await page.waitForSelector('text=Embed on your website');
  const codes = await page.locator('textarea[readonly]').evaluateAll((els) => els.map((e) => (e as HTMLTextAreaElement).value));
  if (!codes.some((c) => c.includes(`data-form="${key}"`) && c.includes('/embed.js'))) throw new Error('embed code missing: ' + codes.join(' | '));
});

await step('hosted page: submit creates a lead', async () => {
  const p2 = await page.context().newPage();
  await p2.goto(`${B}/f/${key}?utm_source=newsletter&utm_campaign=nov`);
  await p2.waitForSelector('h1:text-is("Book a demo")');
  await sleep(2200);
  await p2.fill('#nf-f1', `Hosted${s}`);
  await p2.fill('#nf-f2', `hosted.${s}@example.com`);
  await p2.selectOption('#nf-f5', '11-100');
  await p2.click('button:has-text("Send")');
  await p2.waitForSelector('text=Thank you! We will be in touch soon.');
  await p2.close();
  const subs = await api('GET', `/crm/webforms/${formId}/submissions`, null, token);
  if (subs.items[0]?.utm?.source !== 'newsletter') throw new Error('utm missing: ' + JSON.stringify(subs.items[0]));
});

await step('embedded on another site: resizes, keeps host-page UTM', async () => {
  const site = http.createServer((req, res) => { res.writeHead(200, { 'content-type': 'text/html' }); res.end(`<!doctype html><title>Acme site</title><h1>Welcome to Acme</h1><script src="${B}/embed.js" data-form="${key}" async></script>`); });
  await new Promise<void>((r) => site.listen(8097, '127.0.0.1', () => r()));
  const p3 = await page.context().newPage();
  await p3.goto('http://127.0.0.1:8097/landing?utm_source=adwords&utm_medium=cpc&utm_campaign=demo');
  const frame = p3.frameLocator('iframe[title="Contact form"]');
  await frame.locator('h1:text-is("Book a demo")').waitFor();
  await sleep(600);
  const h = await p3.locator('iframe').evaluate((el) => el.getBoundingClientRect().height);
  if (h < 400) throw new Error('iframe did not grow: ' + h);
  await sleep(2000);
  await frame.locator('#nf-f1').fill(`Embedded${s}`);
  await frame.locator('#nf-f2').fill(`embedded.${s}@example.com`);
  await frame.locator('button:has-text("Send")').click();
  await frame.locator('text=Thank you!').waitFor();
  await p3.screenshot({ path: '25-webform-embedded.png', fullPage: true });
  await p3.close(); site.close();
  const subs = await api('GET', `/crm/webforms/${formId}/submissions`, null, token);
  const e = subs.items.find((x) => x.data.some((d) => d.value === `Embedded${s}`));
  if (e?.utm?.source !== 'adwords' || !e.page_url?.startsWith('http://127.0.0.1:8097/landing')) throw new Error('host page not recorded: ' + JSON.stringify(e));
});

await step('lead shows the form answers and UTM', async () => {
  const subs = await api('GET', `/crm/webforms/${formId}/submissions`, null, token);
  const e = subs.items.find((x) => x.data.some((d) => d.value === `Embedded${s}`));
  await page.goto(`${B}/crm/lead/${e.item_id}`);
  await page.waitForSelector(`text=submitted the form “Demo request ${s}”`);
  await page.waitForSelector('text=source: adwords');
  await page.screenshot({ path: '26-webform-lead.png', fullPage: true });
});

await step('cleanup', async () => { await api('DELETE', `/crm/webforms/${formId}`, null, token); });

console.log(errors.length ? 'CONSOLE ERRORS:\n' + errors.join('\n') : 'no console errors');
await browser.close();
await cleanupRun(s);
process.exit(failed || errors.length ? 1 : 0);
