// Edit history, observers, tasks of a record, task ↔ CRM links, email templates in the UI.
import { chromium } from 'playwright';
import { cleanupRun } from './cleanup.ts';
const B = 'http://localhost:8080';
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: any[] = [];
const page = await (await browser.newContext({ viewport: { width: 1440, height: 1000 } })).newPage();
page.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
page.on('console', (m) => m.type() === 'error' && !/401|400|Failed to load resource/.test(m.text()) && errors.push(m.text()));
const dialogs: any[] = [];
page.on('dialog', (d) => { const v = dialogs.shift(); v === false ? d.dismiss() : d.accept(typeof v === 'string' ? v : undefined); });
let n = 0, failed = 0;
const step = async (name, fn) => { try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); await page.screenshot({ path: `fail18-${++n}.png`, fullPage: true }); } };
const s = Date.now().toString(36);
const api = async (method: string, path: string, body?: any, token?: string | null) => (await fetch(B + '/api' + path, { method, headers: { 'content-type': 'application/json', ...(token ? { authorization: `Bearer ${token}` } : {}) }, body: body ? JSON.stringify(body) : undefined })).json();
const token = (await api('POST', '/auth/login', { email: 'admin@acme.test', password: 'supersecret1' })).access_token;
const cfg = await api('GET', '/crm/config', null, token);
const peer = await api('POST', '/users', { email: `peer.${s}@acme.test`, password: 'password123', first_name: 'Pia', last_name: `Peer${s}` }, token);
const contact = await api('POST', '/crm/contact/items', { first_name: 'Cleo', last_name: `Client${s}`, multifields: [{ kind: 'email', value_type: 'work', value: `cleo.${s}@example.com` }] }, token);
const deal = await api('POST', '/crm/deal/items', { title: `Collab ${s}`, opportunity: '500', currency: cfg.base_currency, contact_ids: [contact.id] }, token);
const mb = await api('POST', '/crm/mailboxes', { email: 'sales@nerp.test', password: 'nerptest', imap_host: '127.0.0.1', imap_port: 143, imap_security: 'none', smtp_host: '127.0.0.1', smtp_port: 1025, smtp_security: 'starttls', smtp_auth: false, allow_invalid_certs: true }, token);

await page.goto(B + '/login'); await page.fill('#email', 'admin@acme.test'); await page.fill('#password', 'supersecret1');
await page.click('button:has-text("Sign in")'); await page.waitForURL(B + '/');

await step('edit history in the timeline', async () => {
  await page.goto(B + `/crm/deal/${deal.id}`);
  await page.click('button:has-text("Edit")');
  await page.fill('#crm-amount', '750');
  await page.fill('#crm-close', '2027-05-20');
  await page.click('form button:has-text("Save")');
  const h = page.locator('[data-history]').first();
  await h.locator('text=Amount:').waitFor();
  await h.locator('text=500').waitFor();
  await h.locator('text=750').waitFor();
  await h.locator('text=Expected close date:').waitFor();
  await page.locator('text=changed Amount, Expected close date').first().waitFor();
});

await step('observers: add and remove', async () => {
  await page.fill('[aria-label="Observers"] input[placeholder="+ Add observer"]', `Peer${s}`);
  await page.click(`button:has-text("Pia Peer${s}")`);
  await page.locator(`[aria-label="Observers"] >> text=Pia Peer${s}`).waitFor();
  await page.locator(`text=added Pia Peer${s} as observer`).waitFor();
  const d = await api('GET', `/crm/deal/items/${deal.id}`, null, token);
  if (d.observers[0]?.id !== peer.id) throw new Error('not saved');
  await page.click(`button[aria-label="Remove observer Pia Peer${s}"]`);
  await page.locator('[aria-label="Observers"] >> text=none').waitFor();
});

let taskUrl;
await step('a task from the record', async () => {
  await page.locator('section[aria-labelledby="tasks-h"] >> text=No tasks yet').waitFor();
  await page.click('section[aria-labelledby="tasks-h"] button:has-text("Task")');
  await page.fill('#nt-title', `Send pricing ${s}`);
  await page.click('dialog[open] button:has-text("Create task")');
  await page.waitForSelector('text=Task created');
  const link = page.locator(`section[aria-labelledby="tasks-h"] a:has-text("Send pricing ${s}")`);
  await link.waitFor();
  await page.locator(`text=created the task #`).waitFor();
  taskUrl = B + (await link.getAttribute('href'));
});

await step('the task shows its CRM records; relink', async () => {
  await page.goto(taskUrl);
  const sec = page.locator('section[aria-labelledby="crm-h"]');
  await sec.locator(`a:has-text("Collab ${s}")`).waitFor();
  await sec.locator(`input[placeholder^="Link a deal"]`).fill(`Client${s}`);
  await page.click(`button:has-text("Cleo Client${s}")`);
  await sec.locator(`a:has-text("Cleo Client${s}")`).waitFor();
  await sec.locator(`a:has-text("Collab ${s}")`).hover();
  await sec.locator(`button[aria-label="Unlink Collab ${s}"]`).click();
  await page.waitForFunction((t) => !document.querySelector('section[aria-labelledby="crm-h"]')!.textContent!.includes(t), `Collab ${s}`);
});

await step('email templates: save, use, delete', async () => {
  await page.goto(B + `/crm/deal/${deal.id}`);
  await page.click('button:has-text("Email")');
  await page.locator('select[aria-label="Email template"]').waitFor();
  await page.fill('#em-subject', 'Offer for {{title}}');
  await page.fill('textarea[aria-label="Message"]', 'Dear {{contact}},\nhere it is.\n{{me.name}}');
  dialogs.push(`Offer ${s}`, true); // name, then "share with everyone"
  await page.click('button:has-text("Save as template")');
  await page.waitForSelector('text=Template saved');
  await page.fill('#em-subject', '');
  await page.fill('textarea[aria-label="Message"]', '');
  await page.selectOption('select[aria-label="Email template"]', { label: `Offer ${s}` });
  await page.waitForFunction((t) => document.querySelector<HTMLInputElement>('#em-subject')!.value === `Offer for Collab ${t}`, s);
  const body = await page.inputValue('textarea[aria-label="Message"]');
  if (!body.startsWith(`Dear Cleo Client${s},`) || !body.includes('Ada Admin')) throw new Error(body);
  const list = await api('GET', '/crm/email-templates', null, token);
  if (list.items.find((t) => t.name === `Offer ${s}`)?.owner_id !== null) throw new Error('not shared');
  dialogs.push(true);
  await page.click('button:has-text("Delete template")');
  await page.waitForFunction((t) => ![...document.querySelectorAll('select[aria-label="Email template"] option')].some((o) => o.textContent === `Offer ${t}`), s);
});

await page.screenshot({ path: '37-collab.png', fullPage: true });
for (const t of (await api('GET', `/crm/items/${contact.id}/tasks`, null, token)).items ?? []) await api('DELETE', `/tasks/${t.id}`, null, token);
for (const [c, id] of [['deal', deal.id], ['contact', contact.id]]) { await api('DELETE', `/crm/${c}/items/${id}`, null, token); await api('DELETE', `/crm/recycle-bin/${id}`, null, token); }
if (mb?.id) await api('DELETE', `/crm/mailboxes/${mb.id}`, null, token);
console.log(errors.length ? 'console errors:\n' + errors.join('\n') : 'no console errors');
await browser.close();
await cleanupRun(s);
process.exit(failed || errors.length ? 1 : 0);
