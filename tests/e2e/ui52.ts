// Help desk in the browser: a manager sets up a category (with who it goes to), an SLA and a
// frequently used answer; an employee asks for help; support writes a note, answers with the
// saved answer and changes the ticket; the employee closes and rates it; a customer uses the
// public help page and their private request page; support reads the statistics.
import { chromium } from 'playwright';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
const B = 'http://localhost:8080';
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: string[] = [];
const s = Date.now().toString(36);
let n = 0, failed = 0;
const pages: import('playwright').Page[] = [];
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `fail52-${n}-${i}.png`, fullPage: true }); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const employee = (await admin.get('/roles')).data.find((x: any) => x.code === 'employee');
const sueUser = (await admin.post('/users', { email: `sue.${s}@acme.test`, password: 'password123', first_name: 'Sue', last_name: 'Helper' })).data;
const role = (await admin.post('/roles', { code: `desk_${s}`, name: `Desk ${s}`, permissions: ['support.work'] })).data;
await admin.put(`/users/${sueUser.id}/roles`, { role_ids: [employee.id, role.id] });
await admin.post('/users', { email: `emma.${s}@acme.test`, password: 'password123', first_name: 'Emma', last_name: 'Asker' });

const newPage = async () => {
  const p = await (await browser.newContext({ viewport: { width: 1440, height: 1000 }, timezoneId: 'UTC', locale: 'en-GB' })).newPage();
  p.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
  p.on('console', (m) => m.type() === 'error' && !/401|400|404|409|Failed to load resource/.test(m.text()) && errors.push(m.text()));
  pages.push(p);
  return p;
};
const ctx = async (email: string, password: string) => {
  const p = await newPage();
  await p.goto(B + '/login'); await p.fill('#email', email); await p.fill('#password', password);
  await p.click('button:has-text("Sign in")'); await p.waitForURL(B + '/');
  return p;
};
const category = `Hardware ${s}`;
const title = `Laptop will not start ${s}`;

try {
  const ada = await ctx('admin@acme.test', 'supersecret1');
  const sue = await ctx(`sue.${s}@acme.test`, 'password123');
  const emma = await ctx(`emma.${s}@acme.test`, 'password123');

  await step('a category that goes to Sue', async () => {
    await ada.click('nav >> text=Help desk');
    await ada.click('a[href="/support/settings"]');
    await ada.click('[role=tab]:has-text("Lists")');
    await ada.fill('[data-kind="category"] input[aria-label="New categories entry"]', category);
    await ada.click('[data-kind="category"] button:has-text("Add")');
    const row = ada.locator(`[data-kind="category"] li[data-entry="${category}"]`);
    await row.waitFor();
    await row.locator('select[aria-label="Goes to"]').selectOption({ label: 'Sue Helper' });
    await ada.waitForTimeout(300);
    await ada.reload();
    await ada.click('[role=tab]:has-text("Lists")');
    if (await ada.locator(`[data-kind="category"] li[data-entry="${category}"] select`).inputValue() !== sueUser.id) throw new Error('not saved');
  });

  await step('an SLA for it: 4 hours, weekdays and weekends round the clock', async () => {
    await ada.click('[role=tab]:has-text("SLAs")');
    await ada.click('button:has-text("New SLA")');
    await ada.fill('#sla-name', `Hardware SLA ${s}`);
    await ada.fill('#sla-prio', '5');
    await ada.fill('#sla-resp', '240');
    await ada.fill('#sla-notice', '30');
    await ada.check(`fieldset:has(legend:has-text("Categories")) label:has-text("${category}") input`);
    for (const d of [1, 2, 3, 4, 5, 6, 7]) {
      const day = ada.locator(`[data-day="${d}"]`);
      await day.locator('input[type=checkbox]').check();
      await day.locator('input[type=time]').nth(0).fill('00:00');
      await day.locator('input[type=time]').nth(1).fill('00:00');
    }
    await ada.click('button[form="sla-form"]');
    await ada.locator(`[data-sla="Hardware SLA ${s}"]:has-text("Reply within 4 h")`).waitFor();
  });

  await step('a frequently used answer', async () => {
    await ada.click('[role=tab]:has-text("Frequently used answers")');
    await ada.click('button:has-text("New answer")');
    await ada.fill('#a-title', `Power reset ${s}`);
    await ada.fill('#a-body', 'Hold the power button for 30 seconds, then start it again.');
    await ada.click('button[form="answer-form"]');
    await ada.locator(`[data-answer="Power reset ${s}"]`).waitFor();
  });

  let ticketUrl = '';
  await step('an employee asks for help', async () => {
    await emma.click('nav >> text=Help desk');
    await emma.locator('text=You have not asked for help yet.').waitFor();
    await emma.click('button:has-text("New request")');
    if (await emma.locator('legend:has-text("For")').count()) throw new Error('employees only ask for themselves');
    await emma.fill('#t-title', title);
    await emma.selectOption('#t-cat', { label: category });
    await emma.fill('#t-body', 'It shows a black screen.');
    await emma.click('button[form="new-ticket"]');
    await emma.waitForURL(/\/support\/tickets\/[0-9a-f-]+$/);
    ticketUrl = emma.url();
    await emma.locator('[data-state]:has-text("Waiting for support")').waitFor();
    await emma.locator('dd:has-text("Sue Helper")').waitFor();
  });

  await step('support sees it with its deadline', async () => {
    await sue.click('nav >> text=Help desk');
    const row = sue.locator(`tr:has-text("${title}")`);
    await row.locator('[data-due]', { hasText: /due in [34] h/ }).waitFor();
    await row.locator('a').click();
    await sue.locator('[data-deadline]', { hasText: /Reply due in [34] h/ }).waitFor();
    if (await sue.locator('#f-sla').inputValue() === '') throw new Error('no SLA');
    if (!(await sue.locator('#f-sla option:checked').textContent())?.includes(`Hardware SLA ${s}`)) throw new Error('wrong SLA');
  });

  await step('a note for support only', async () => {
    await sue.check('label:has-text("Note") input');
    await sue.fill('textarea[aria-label="Note"]', `Probably the battery ${s}`);
    await sue.click('button:has-text("Add note")');
    await sue.locator(`[data-message="note"]:has-text("Probably the battery ${s}")`).waitFor();
  });

  await step('an answer from the saved ones, with time spent and a status', async () => {
    await sue.selectOption('select[aria-label="Insert a frequently used answer"]', { label: `Power reset ${s}` });
    if (!(await sue.inputValue('textarea[aria-label="Reply"]')).includes('Hold the power button')) throw new Error('answer not inserted');
    await sue.fill('input[aria-label="Minutes spent"]', '10');
    await sue.selectOption('select[aria-label="Set status"]', { label: 'Problem solving in progress' });
    await sue.click('button:text-is("Send")');
    await sue.locator('[data-state]:has-text("Waiting for the customer")').waitFor();
    await sue.locator('[data-spent]:has-text("10 min")').waitFor();
    await sue.locator('[data-log]:has-text("Status: Request accepted → Problem solving in progress")').waitFor();
  });

  await step('changing the criticality is kept in the history', async () => {
    await sue.selectOption('#f-criticality', { label: 'High' });
    await sue.locator('[data-log]:has-text("Criticality: Middle → High")').waitFor();
  });

  await step('the employee sees the answer, not the note, then closes and rates it', async () => {
    await emma.goto(ticketUrl);
    await emma.locator('[data-message="message"]:has-text("Hold the power button")').waitFor();
    if (await emma.locator(`text=Probably the battery ${s}`).count()) throw new Error('the note is visible');
    if (await emma.locator('[data-log]').count()) throw new Error('the history is visible');
    await emma.click('aside button:has-text("Close")');
    await emma.locator('[data-rate]').waitFor();
    await emma.click('[data-rate] button:has-text("Answer suits the needs")');
    await emma.locator('[data-rate] button.btn-primary:has-text("Answer suits the needs")').waitFor();
  });

  let customerPage: import('playwright').Page;
  await step('a customer uses the public help page', async () => {
    customerPage = await newPage();
    const c = customerPage;
    await c.goto(B + '/help');
    await c.fill('#h-name', 'Carl Customer');
    await c.fill('#h-email', `carl.${s}@example.com`);
    await c.fill('#h-title', `Order missing ${s}`);
    await c.selectOption('#h-cat', { label: category });
    await c.fill('#h-body', 'My order has not arrived.');
    await c.click('button:has-text("Send")');
    await c.waitForURL(/\/help\/t\/[^/?]+\?new=\d+$/);
    await c.locator('[role=status]:has-text("Thank you! Your request number is")').waitFor();
    await c.locator('[data-status]:has-text("We are working on it")').waitFor();
  });

  await step('support answers it; the customer sees the answer and writes back', async () => {
    await sue.goto(B + '/support');
    await sue.click(`a:has-text("Order missing ${s}")`);
    await sue.fill('textarea[aria-label="Reply"]', 'It ships tomorrow.');
    await sue.click('button:text-is("Send")');
    await sue.locator('[data-state]:has-text("Waiting for the customer")').waitFor();
    const c = customerPage!;
    await c.reload();
    await c.locator('[data-message]:has-text("It ships tomorrow.")').waitFor();
    await c.locator('[data-status]:has-text("Answered")').waitFor();
    await c.fill('#r-body', 'Great, thanks!');
    await c.click('button:has-text("Send")');
    await c.locator('[data-message]:has-text("Great, thanks!")').waitFor();
  });

  await step('the customer closes and rates it', async () => {
    const c = customerPage!;
    await c.click('button:has-text("My problem is solved")');
    await c.locator('[data-status]:has-text("Closed")').waitFor();
    await c.click('[data-rate] button:has-text("Answer suits the needs")');
    await c.locator('text=Thank you for your rating.').waitFor();
  });

  await step('the statistics', async () => {
    await sue.goto(B + '/support');
    await sue.click('a[href="/support/stats"]');
    await sue.locator('[data-stat="Created"]').waitFor();
    if (Number(await sue.locator('[data-stat="Created"] p.text-2xl').textContent()) < 2) throw new Error('too few created');
    await sue.locator('tr[data-person="Sue Helper"]').waitFor();
    await sue.click('button:has-text("Show as table")');
    await sue.locator('table:has(th:has-text("Closed")) tbody tr').first().waitFor();
  });
} finally {
  await browser.close();
  await cleanupRun(s);
}
console.log(errors.length ? 'console errors:\n' + errors.join('\n') : 'no console errors');
process.exit(failed || errors.length ? 1 : 0);
