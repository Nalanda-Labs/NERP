// Email UI: connect a mailbox, incoming email on a deal, reply from the record.
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
const step = async (name, fn) => { try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); await page.screenshot({ path: `fail10-${++n}.png`, fullPage: true }); } };
const s = Date.now().toString(36);

import { execSync } from 'node:child_process';
import { writeFileSync } from 'node:fs';
import { cleanupRun } from './cleanup.ts';
const api = async (method: string, path: string, body?: any, token?: string | null) => (await fetch(B + '/api' + path, { method, headers: { 'content-type': 'application/json', ...(token ? { authorization: `Bearer ${token}` } : {}) }, body: body ? JSON.stringify(body) : undefined })).json();
const token = (await api('POST', '/auth/login', { email: 'admin@acme.test', password: 'supersecret1' })).access_token;
for (const m of (await api('GET', '/crm/mailboxes', null, token)).items) await api('DELETE', `/crm/mailboxes/${m.id}`, null, token);
const client = `ui.client.${s}@example.com`;
const contact = await api('POST', '/crm/contact/items', { first_name: 'Una', last_name: `Inbox${s}`, multifields: [{ kind: 'email', value: client }] }, token);
const deal = await api('POST', '/crm/deal/items', { title: `UI mail deal ${s}`, contact_ids: [contact.id] }, token);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

await page.goto(B + '/login'); await page.fill('#email', 'admin@acme.test'); await page.fill('#password', 'supersecret1');
await page.click('button:has-text("Sign in")'); await page.waitForURL(B + '/');

await step('connect a mailbox with a connection test', async () => {
  await page.click('aside >> text=Mailboxes');
  await page.click('button:has-text("Other mailbox (IMAP)")');
  await page.fill('#mb-email', 'sales@nerp.test');
  await page.fill('#mb-name', 'Ada at Acme');
  await page.fill('#mb-pass', 'nerptest');
  await page.fill('#mb-ih', '127.0.0.1'); await page.fill('#mb-ip', '143'); await page.selectOption('#mb-is', 'none');
  await page.fill('#mb-sh', '127.0.0.1'); await page.fill('#mb-sp', '1025'); await page.selectOption('#mb-ss', 'starttls');
  await page.uncheck('text=The outgoing server needs the login');
  await page.check('text=Accept self-signed certificates');
  await page.click('button:has-text("Test connection")');
  await page.waitForSelector('text=Incoming (IMAP): connected');
  await page.waitForSelector('text=Outgoing (SMTP): connected');
  await page.screenshot({ path: '20-mailbox.png' });
  await page.click('dialog >> button:text-is("Connect")');
  await page.waitForSelector('li:has-text("sales@nerp.test")');
  await page.waitForSelector('text=Checked', { timeout: 20000 }).catch(async () => { await page.reload(); await page.waitForSelector('text=Checked', { timeout: 15000 }); });
});

await step('incoming email appears on the deal and opens', async () => {
  const f = `nerp-ui-${s}.eml`;
  writeFileSync(f, [`From: Una Inbox <${client}>`, 'To: sales@nerp.test', `Subject: Quote please ${s}`, `Message-ID: <ui1.${s}@example.com>`, `Date: ${new Date().toUTCString()}`,
    'Content-Type: text/html; charset=utf-8', '', `<p>Could you send a <b>quote</b> for 20 seats?</p><img src="https://tracker.example/x.gif">`].join('\r\n'));
  execSync(`curl -s imap://127.0.0.1/INBOX -u sales@nerp.test:nerptest -T ${f}`);
  const mbs = await api('GET', '/crm/mailboxes', null, token);
  await api('POST', `/crm/mailboxes/${mbs.items[0].id}/sync`, {}, token);
  await page.goto(B + `/crm/deal/${deal.id}`);
  await page.waitForSelector(`text=${client} wrote an email`, { timeout: 20000 }).catch(async () => { await sleep(3000); await page.reload(); await page.waitForSelector(`text=${client} wrote an email`); });
  await page.click(`button:has-text("Quote please ${s}")`);
  const frame = page.frameLocator('iframe[title="Email body"]');
  await frame.locator('b:text-is("quote")').waitFor();
  await page.screenshot({ path: '21-email-open.png', fullPage: true });
});

await step('reply from the record', async () => {
  await page.click('button:has-text("Reply")');
  if ((await page.inputValue('#em-to')) !== client) throw new Error('reply to not prefilled');
  if ((await page.inputValue('#em-subject')) !== `Re: Quote please ${s}`) throw new Error('subject not prefilled');
  await page.fill('textarea[aria-label="Message"]', 'Hello Una,\nthe quote follows.\nAda');
  await page.click('button:has-text("Send")');
  await page.waitForSelector('text=Email is being sent');
  await page.waitForSelector('text=sent an email', { timeout: 20000 });
  let found;
  for (let i = 0; i < 30 && !found; i++) { found = (await (await fetch(`http://localhost:8026/api/v1/search?query=${encodeURIComponent(`subject:"Re: Quote please ${s}"`)}`)).json()).messages?.[0]; if (!found) await sleep(500); }
  if (!found) throw new Error('not delivered');
  await page.screenshot({ path: '22-email-replied.png', fullPage: true });
});

await step('cleanup', async () => {
  for (const m of (await api('GET', '/crm/mailboxes', null, token)).items) await api('DELETE', `/crm/mailboxes/${m.id}`, null, token);
});

console.log(errors.length ? 'CONSOLE ERRORS:\n' + errors.join('\n') : 'no console errors');
await browser.close();
await cleanupRun(s);
process.exit(failed || errors.length ? 1 : 0);
