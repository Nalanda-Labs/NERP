// Task depth in the browser: a required custom field in the task form, a checklist point
// given to someone, marking a comment as the result, rating, new comments, the calendar and
// planner views, and the recycle bin.
import { chromium } from 'playwright';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
const B = 'http://localhost:8080';
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: string[] = [];
const s = Date.now().toString(36);
let n = 0, failed = 0;
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); await p.screenshot({ path: `fail47-${++n}.png`, fullPage: true }); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const bob = (await admin.post('/users', { email: `bob.${s}@acme.test`, password: 'password123', first_name: 'Bob', last_name: `Tasks${s}` })).data;
const bobApi = new Client();
await bobApi.post('/auth/login', { email: `bob.${s}@acme.test`, password: 'password123' });
const field = (await admin.post('/tasks/fields', { name: `Client ${s}`, kind: 'select', options: ['Acme', 'Globex'], required: true })).data.items.find((f: any) => f.name === `Client ${s}`);

const p = await (await browser.newContext({ viewport: { width: 1440, height: 1000 } })).newPage();
p.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
p.on('console', (m) => m.type() === 'error' && !/401|400|404|Failed to load resource/.test(m.text()) && errors.push(m.text()));
await p.goto(B + '/login'); await p.fill('#email', 'admin@acme.test'); await p.fill('#password', 'supersecret1');
await p.click('button:has-text("Sign in")'); await p.waitForURL(B + '/');
let taskUrl = '';

try {
  await step('the task form asks for a required field', async () => {
    await p.goto(B + '/tasks');
    await p.click('button:has-text("New task")');
    await p.fill('input[placeholder="What needs to be done?"]', `Quarterly report ${s}`);
    await p.selectOption(`#tf-field-${field.id}`, 'Globex');
    await p.click('button:has-text("More options")');
    await p.check('label:has-text("Needs a result") input');
    await p.click('button:has-text("Create task")');
    await p.waitForURL(/\/tasks\/[0-9a-f-]+$/);
    taskUrl = p.url();
    await p.locator(`[data-field="Client ${s}"]:has-text("Globex")`).waitFor();
    await p.locator('text=This task needs a result').waitFor();
  });

  await step('a checklist point for Bob', async () => {
    await p.fill('input[placeholder="Add an item…"]', 'Collect numbers');
    await p.click('button:has-text("Add")');
    const row = p.locator('li:has-text("Collect numbers")');
    await row.hover();
    await row.locator('button[aria-label="Assign Collect numbers"]').click();
    await p.locator('dialog[open] input').fill('Bob');
    await p.locator(`dialog[open] >> text=Bob Tasks${s}`).first().click();
    await row.locator(`[data-assignee]:has-text("Bob Tasks${s}")`).waitFor();
  });

  await step('mark a comment as the result, then finish and rate', async () => {
    await p.fill('textarea[placeholder^="Write a comment"]', `Sent to the board ${s}`);
    await p.click('button:text-is("Comment")');
    const c = p.locator(`li:has-text("Sent to the board ${s}")`);
    await c.hover();
    await c.locator('button:has-text("Mark as result")').click();
    await p.locator(`[data-result]:has-text("Sent to the board ${s}")`).waitFor();
    await p.click('button:text-is("Complete")');
    await p.locator('[data-rating]').waitFor();
    await p.click('button:has-text("👍 Well")');
    await p.locator('[data-rating="good"]').waitFor();
  });

  await step('new comments show in the list', async () => {
    const t = (await bobApi.post('/tasks', { title: `For Ada ${s}`, responsible_id: (await admin.get('/auth/me')).data.id ?? (await admin.get('/auth/me')).data.user.id })).data;
    await bobApi.post(`/tasks/${t.id}/comments`, { body: 'Please look' });
    await p.goto(B + '/tasks?view=my&unread=true');
    await p.locator(`tr:has-text("For Ada ${s}") [data-new-comments]:has-text("1 new")`).waitFor();
  });

  await step('calendar view', async () => {
    const d = new Date(); d.setHours(12, 0, 0, 0);
    await admin.post('/tasks', { title: `Due today ${s}`, deadline: d.toISOString() });
    await p.goto(B + '/tasks?layout=calendar&view=my');
    await p.locator(`[data-cal-task="Due today ${s}"]`).waitFor();
  });

  await step('my planner', async () => {
    await p.goto(B + '/tasks?layout=planner');
    const card = p.locator(`[data-card]:has-text("Due today ${s}")`);
    await card.waitFor();
    // Wait for the move to be saved: reloading at once would cancel the request.
    await Promise.all([
      p.waitForResponse((r) => /\/api\/tasks\/[0-9a-f-]+\/planner$/.test(r.url()) && r.request().method() === 'POST' && r.ok()),
      card.dragTo(p.locator('[data-column]').nth(2)),
    ]);
    await p.reload();
    await p.locator('[data-column]').nth(2).locator(`[data-card]:has-text("Due today ${s}")`).waitFor();
  });

  await step('delete and restore from the bin', async () => {
    await p.goto(taskUrl);
    p.once('dialog', (d) => d.accept());
    await p.click('button[aria-label="Delete task"]');
    await p.waitForURL((u) => !u.pathname.startsWith('/tasks/') || u.pathname === '/tasks');
    await p.goto(B + '/tasks/trash');
    await p.locator(`[data-binned="Quarterly report ${s}"] button:has-text("Restore")`).click();
    await p.waitForURL(taskUrl);
    await p.locator(`[data-result]:has-text("Sent to the board ${s}")`).waitFor();
  });
} finally {
  await browser.close();
  await cleanupRun(s);
}
console.log(errors.length ? 'console errors:\n' + errors.join('\n') : 'no console errors');
process.exit(failed || errors.length ? 1 : 0);
