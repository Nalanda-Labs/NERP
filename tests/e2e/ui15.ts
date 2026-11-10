// Automation depth in the UI: date-timed and waiting robots, order, stage time limits on the
// board and record, schedules, the log with retry, and time in stages.
import { chromium } from 'playwright';
import { execSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { cleanupRun } from './cleanup.ts';
const B = 'http://localhost:8080';
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: any[] = [];
const ctx = await browser.newContext({ viewport: { width: 1440, height: 1000 } });
const page = await ctx.newPage();
page.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
page.on('console', (m) => m.type() === 'error' && !/401|400|Failed to load resource/.test(m.text()) && errors.push(m.text()));
page.on('dialog', (d) => d.accept());
let n = 0, failed = 0;
const step = async (name, fn) => { try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); await page.screenshot({ path: `fail15-${++n}.png`, fullPage: true }); } };
const s = Date.now().toString(36);
const api = async (method: string, path: string, body?: any, token?: string | null) => (await fetch(B + '/api' + path, { method, headers: { 'content-type': 'application/json', ...(token ? { authorization: `Bearer ${token}` } : {}) }, body: body ? JSON.stringify(body) : undefined })).json();
const token = (await api('POST', '/auth/login', { email: 'admin@acme.test', password: 'supersecret1' })).access_token;
const dbUrl = process.env.DATABASE_URL ?? readFileSync(new URL('../../backend/.env', import.meta.url), 'utf8').match(/^DATABASE_URL=(.*)$/m)?.[1];
const sql = (q) => execSync(`psql ${dbUrl} -Atc "${q}"`).toString().trim();
const cat = (await api('POST', '/crm/deal/categories', { name: `Flow ${s}` }, token)).categories.find((c) => c.name === `Flow ${s}`);
const st = Object.fromEntries(cat.stages.map((x) => [x.name, x.id]));
const ids: any[] = [];
const mkDeal = async (b) => { const d = await api('POST', '/crm/deal/items', { category_id: cat.id, ...b }, token); ids.push(d.id); return d; };
const late = await mkDeal({ title: `Late ${s}` });
await api('PATCH', `/crm/deal/items/${late.id}`, { stage_id: st.Preparation }, token);
sql(`UPDATE crm_items SET stage_changed_at = now() - interval '3 hours' WHERE id = '${late.id}'`);
const overdue = await mkDeal({ title: `Overdue ${s}`, close_date: new Date(Date.now() - 5 * 86400000).toISOString().slice(0, 10) });
// A robot that fails (no client email), for the log.
await api('POST', '/crm/robots', { category_id: cat.id, stage_id: st['Proposal sent'], name: 'Mail client', action: 'email', params: { subject: 'Hi', body: 'Hello' } }, token);
const nomail = await mkDeal({ title: `No mail ${s}` });
await api('PATCH', `/crm/deal/items/${nomail.id}`, { stage_id: st['Proposal sent'] }, token);

await page.goto(B + '/login'); await page.fill('#email', 'admin@acme.test'); await page.fill('#password', 'supersecret1');
await page.click('button:has-text("Sign in")'); await page.waitForURL(B + '/');
await page.goto(B + '/crm/settings?tab=automation');
await page.selectOption('select[aria-label="Pipeline"]', cat.id);
const col = (name) => page.locator(`section[aria-label="Stage ${name}"]`);

await step('robot timed from a date that waits for a reply', async () => {
  await col('New').locator('button:has-text("+ Robot")').click();
  await page.selectOption('#au-action', 'webhook');
  await page.fill('#au-name', 'Notify ERP');
  await page.fill('#au-url', 'https://erp.example.com/hooks/{{id}}');
  await page.selectOption('select[aria-label="When"]', 'field');
  await page.fill('input[aria-label="Offset"]', '2');
  await page.selectOption('select[aria-label="Offset unit"]', '1440');
  await page.selectOption('select[aria-label="Before or after"]', 'before');
  await page.selectOption('select[aria-label="Date field"]', 'close_date');
  await page.check('text=Wait before acting');
  await page.selectOption('select[aria-label="Wait until"]', 'email_received');
  await page.fill('input[aria-label="Timeout"]', '3');
  await page.selectOption('select[aria-label="Timeout unit"]', '1440');
  await page.check('input[type=radio][value=skip]');
  await page.click('dialog button:has-text("Save")');
  await page.waitForSelector('text=Automation saved');
  const card = col('New').locator('div.group:has-text("Notify ERP")');
  await card.locator('text=2 d before Expected close date').waitFor();
  await card.locator('text=waits for a client reply, 3 d max').waitFor();
  const r = (await api('GET', `/crm/automation?category_id=${cat.id}`, null, token)).robots.find((x) => x.name === 'Notify ERP');
  if (r.timing !== 'field' || r.offset_minutes !== -2880 || r.wait?.until !== 'email_received' || r.wait.timeout_minutes !== 4320 || r.wait.on_timeout !== 'skip') throw new Error(JSON.stringify(r));
});

await step('second robot runs after it; reorder', async () => {
  await col('New').locator('button:has-text("+ Robot")').click();
  await page.selectOption('#au-action', 'comment');
  await page.fill('#au-name', 'Log it');
  await page.fill('#au-text', 'Sent to the ERP');
  await page.check('text=Start only after the robot above it has finished');
  await page.click('dialog button:has-text("Save")');
  await page.waitForSelector('text=Automation saved');
  await col('New').locator('text=↓ then').waitFor();
  const first = col('New').locator('div.group:has-text("Notify ERP")');
  await first.hover();
  await first.locator('button[aria-label="Move down"]').click();
  await page.waitForFunction(() => document.querySelector('section[aria-label="Stage New"] div.group')?.textContent?.includes('Log it'));
  const order = (await api('GET', `/crm/automation?category_id=${cat.id}`, null, token)).robots.filter((x) => x.stage_id === st.New).map((x) => x.name);
  if (order.join() !== 'Log it,Notify ERP') throw new Error(order.join());
});

await step('new actions: linked record, least busy, email with a document', async () => {
  await col('Won').locator('button:has-text("+ Robot")').click();
  await page.selectOption('#au-action', 'create_record');
  await page.fill('#au-ctitle', 'Renewal: {{title}}');
  await page.click('dialog button:has-text("Save")');
  await page.waitForSelector('text=Automation saved');
  await col('Won').locator('text=Deal: Renewal:').waitFor();
  await col('Won').locator('button:has-text("+ Robot")').click();
  await page.selectOption('#au-action', 'email');
  const opts = await page.locator('#au-att option').count();
  if (opts < 2) throw new Error('no templates to attach');
  await page.selectOption('#au-att', { index: 1 });
  await page.locator('text=The document is created first').waitFor();
  await page.click('dialog button:has-text("Cancel")');
  await col('Won').locator('button:has-text("+ Robot")').click();
  await page.selectOption('#au-action', 'assign');
  await page.locator('text=Fewest open records').waitFor();
  if (await page.locator('#au-action option[value=convert_lead]').count()) throw new Error('convert offered for deals');
  await page.click('dialog button:has-text("Cancel")');
});

await step('stage time limit', async () => {
  await col('Preparation').locator('button:has-text("+ Time limit")').click();
  await page.check('text=Limit the time in this stage');
  await page.fill('input[aria-label="Time limit"]', '2');
  await page.selectOption('select[aria-label="Time limit unit"]', '60');
  await page.check('text=Also notify the heads of their departments');
  await page.click('dialog[open] button:has-text("Save")');
  await page.waitForSelector('text=Time limit saved');
  await col('Preparation').locator('text=Time limit 2 h, escalates').waitFor();
  if (await col('Won').locator('text=Time limit').count()) throw new Error('closed stages offer a limit');
});

await step('over the limit on the board and the record', async () => {
  await page.goto(B + '/crm/deal');
  await page.click('button[aria-label="Board"]');
  await page.selectOption('select[aria-label="Pipeline"]', cat.id);
  await page.locator(`a:has-text("Late ${s}") >> text=over the limit`).waitFor();
  if (await page.locator(`a:has-text("Overdue ${s}") >> text=over the limit`).count()) throw new Error('flagged a record without a limit');
  await page.goto(B + (await page.getAttribute(`a:has-text("Late ${s}")`, 'href')));
  await page.locator('text=Over the time limit for “Preparation” (2 h) by 1 h').waitFor();
});

await step('schedule: weekly, run now', async () => {
  await page.goto(B + '/crm/settings?tab=automation');
  await page.selectOption('select[aria-label="Pipeline"]', cat.id);
  await page.click('button[role=tab]:has-text("Schedules")');
  await page.locator('text=No schedules for this pipeline yet').waitFor();
  await page.click('button.btn-primary:has-text("Schedule")');
  await page.fill('#au-name', 'Chase overdue');
  await page.selectOption('#au-to', st['Negotiation']);
  await page.click('button[aria-pressed]:has-text("Fri")');
  await page.fill('input[aria-label="Time"]', '08:30');
  await page.check(`label:has-text("New") >> input[type=checkbox]`);
  await page.click('button:has-text("Add condition")');
  await page.selectOption('select[aria-label="Field"]', 'close_date');
  await page.selectOption('select[aria-label="Operator"]', 'lt');
  await page.locator('text=compare with').waitFor();
  await page.fill('input[aria-label="Value"]', 'today');
  await page.click('dialog button:has-text("Save")');
  await page.waitForSelector('text=Automation saved');
  await page.locator('text=Every Mon, Fri at 08:30').waitFor();
  await page.locator('text=Next:').waitFor();
  await page.click('button:has-text("Run now")');
  await page.waitForSelector('text=Started for 1 record');
  for (let i = 0; i < 40; i++) { if ((await api('GET', `/crm/deal/items/${overdue.id}`, null, token)).stage_id === st['Negotiation']) break; await page.waitForTimeout(250); }
  if ((await api('GET', `/crm/deal/items/${overdue.id}`, null, token)).stage_id !== st['Negotiation']) throw new Error('not moved');
  await page.locator('text=Last:').waitFor();
});

await step('log: filter failed, retry', async () => {
  await page.click('button[role=tab]:has-text("Log")');
  await page.locator(`td >> text=#${nomail.number} No mail ${s}`).first().waitFor();
  await page.click('button[aria-pressed]:has-text("Failed")');
  const row = page.locator(`tr:has-text("No mail ${s}"):has-text("Failed")`).first();
  await row.locator('text=the client has no email address').waitFor();
  await row.locator('button:has-text("Retry")').click();
  await page.waitForSelector('text=runs again');
  await page.waitForFunction((t) => document.querySelectorAll(`tr[data-run]`).length >= 2 && [...document.querySelectorAll('tr[data-run]')].filter((r) => r.textContent.includes(t)).length >= 2, `No mail ${s}`, { timeout: 10000 });
  await page.click('summary:has-text("By robot and schedule")');
  await page.locator('details >> td:has-text("Chase overdue")').waitFor();
  await page.click('details >> button:has-text("Mail client")');
  await page.locator('select[aria-label="Robot or schedule"] >> option:checked:has-text("Mail client")').waitFor({ state: 'attached' });
});

await step('time in stages', async () => {
  await page.click('button[role=tab]:has-text("Time in stages")');
  const prep = page.locator('tr:has-text("Preparation")');
  await prep.locator('text=limit 2 h').waitFor();
  await prep.locator('text=1 now').waitFor();
  await page.locator('tr:has-text("New") td').nth(4).waitFor();
  if (await page.locator('tbody tr:has-text("Won")').count()) throw new Error('closed stages listed');
});

await page.screenshot({ path: '29-automation-log.png', fullPage: true });
await page.goto(B + '/');
const all = (await api('GET', `/crm/deal/items?category_id=${cat.id}&limit=200`, null, token)).items.map((d) => d.id);
for (const id of new Set([...ids, ...all])) { await api('DELETE', `/crm/deal/items/${id}`, null, token); await api('DELETE', `/crm/recycle-bin/${id}`, null, token); }
const gone = await fetch(B + `/api/crm/categories/${cat.id}`, { method: 'DELETE', headers: { authorization: `Bearer ${token}` } });
if (gone.status !== 200) errors.push('cleanup: pipeline not removed ' + gone.status);
console.log(errors.length ? 'console errors:\n' + errors.join('\n') : 'no console errors');
await browser.close();
await cleanupRun(s);
process.exit(failed || errors.length ? 1 : 0);
