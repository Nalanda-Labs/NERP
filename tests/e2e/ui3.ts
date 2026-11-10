import { chromium } from 'playwright';
import { cleanupRun } from './cleanup.ts';
const B = 'http://localhost:8080';
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: any[] = [];
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const ctx = async () => {
  const c = await browser.newContext({ viewport: { width: 1400, height: 900 } });
  const p = await c.newPage();
  p.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
  p.on('console', (m) => m.type() === 'error' && !/401|Failed to load resource/.test(m.text()) && errors.push(m.text()));
  return p;
};
let n = 0;
const step = async (name, page, fn) => { try { await fn(); console.log('ok  ', name); } catch (e: any) { console.log('FAIL', name, e.message.split('\n')[0]); await page.screenshot({ path: `fail-${++n}.png`, fullPage: true }); } };
const login = async (p, email, pw) => { await p.goto(B + '/login'); await p.fill('#email', email); await p.fill('#password', pw); await p.click('button:has-text("Sign in")'); await p.waitForURL(B + '/'); };

// fresh colleague via API
const stamp = Date.now().toString(36);
const TASK = `Design landing page ${stamp}`;
const api = async (method: string, path: string, token?: string | null, body?: any) => (await fetch(B + '/api' + path, { method, headers: { 'content-type': 'application/json', ...(token ? { authorization: `Bearer ${token}` } : {}) }, body: body && JSON.stringify(body) })).json();
const adminTok = (await api('POST', '/auth/login', null, { email: 'admin@acme.test', password: 'supersecret1' })).access_token;
const yaraEmail = `yara.${stamp}@acme.test`;
await api('POST', '/users', adminTok, { email: yaraEmail, password: 'password123', first_name: 'Yara', last_name: `Young${stamp.slice(-3)}`, position: 'Designer' });
const yaraName = `Yara Young${stamp.slice(-3)}`;

const admin = await ctx();
const projName = `Launch ${stamp}`;
await step('login, nav shows Tasks + Workgroups', admin, async () => {
  await login(admin, 'admin@acme.test', 'supersecret1');
  await admin.waitForSelector('nav >> text=Tasks'); await admin.waitForSelector('nav >> text=Workgroups');
});
await step('create project via UI', admin, async () => {
  await admin.click('nav >> text=Workgroups'); await admin.click('button:has-text("New project")');
  await admin.fill('#p-name', projName); await admin.fill('#p-desc', 'Product launch for Q4');
  await admin.fill('dialog input[placeholder="Add people…"]', yaraName); await admin.click(`dialog button:has-text("${yaraName}")`);
  await admin.click('dialog button:has-text("Create project")'); await admin.waitForSelector(`h1:has-text("${projName}")`);
});
await step('create task in project with form', admin, async () => {
  await admin.click('button:has-text("New task")');
  await admin.fill('dialog input[placeholder="What needs to be done?"]', TASK);
  await admin.fill('dialog textarea[placeholder^="Add details"]', 'Hero, pricing and FAQ sections.');
  await admin.fill('dialog input[placeholder="Search employees…"]', yaraName); await admin.click(`dialog button:has-text("${yaraName}")`);
  await admin.click('dialog button:has-text("Tomorrow")');
  await admin.fill('#tf-checklist', 'Wireframe\nVisual design\nHand-off');
  await admin.click('text=More options'); await admin.check('dialog input[type=checkbox]');
  await admin.screenshot({ path: '01-task-form.png' });
  await admin.click('dialog button:has-text("Create task")');
  await admin.waitForSelector(`section:has-text("To do") >> text=${TASK}`);
});
await step('second task + drag to In progress on board', admin, async () => {
  await admin.click('button:has-text("New task")'); await admin.fill('dialog input[placeholder="What needs to be done?"]', 'Write announcement');
  await admin.click('dialog button:has-text("Create task")'); await admin.waitForSelector('text=Write announcement');
  const card = admin.locator('a[data-card]:has-text("Write announcement")');
  const col = admin.locator('section:has(h3:has-text("In progress")) [role=list]');
  await card.dragTo(col);
  await admin.waitForSelector('section:has(h3:has-text("In progress")) >> text=Write announcement');
  await sleep(500); await admin.screenshot({ path: '02-board.png' });
});
const yara = await ctx();
await step('assignee sees live notification + tasks badge', yara, async () => {
  await login(yara, yaraEmail, 'password123');
  await yara.waitForSelector('nav a[href="/tasks"] span.ml-auto');
  await yara.click('button[aria-label^="Notifications"]'); await yara.waitForSelector(`text=New task for you: ${TASK}`);
  await yara.screenshot({ path: '03-yara-dashboard.png' });
  await yara.click(`text=New task for you: ${TASK}`); await yara.waitForURL(/\/tasks\/[0-9a-f-]{36}$/);
});
await step('assignee works the task: checklist, mention, timer', yara, async () => {
  await yara.check('li:has-text("Wireframe") input[type=checkbox]');
  await yara.waitForSelector('text=1/3');
  const ta = yara.locator('textarea[placeholder^="Write a comment"]');
  await ta.click(); await ta.pressSequentially('First draft is ready @Ada', { delay: 20 });
  await yara.waitForSelector('ul >> button:has-text("Ada Admin")'); await yara.keyboard.press('Enter');
  await ta.pressSequentially(' please take a look', { delay: 10 });
  await yara.click('button.btn-primary:has-text("Comment")'); await yara.waitForSelector('a:has-text("@Ada Admin")');
  await yara.click('button:has-text("Start timer")'); await yara.waitForSelector('header >> text=#');
  await sleep(1500); await yara.screenshot({ path: '04-task-detail.png', fullPage: true });
  await yara.click('header button[aria-label="Stop timer"]'); await yara.waitForSelector('button:has-text("Start timer")');
});
await step('admin sees the comment live + mention notification', admin, async () => {
  await admin.click(`nav.card >> text=${TASK}`).catch(() => {});
  await admin.goto(B + '/tasks?view=created'); await admin.click(`text=${TASK}`);
  await admin.waitForSelector('text=please take a look');
  await admin.waitForSelector('button[aria-label*="unread"]');
});
await step('assignee finishes -> review; creator accepts', yara, async () => {
  await yara.click('button:has-text("Finish for review")'); await yara.waitForSelector('text=Waiting for Ada Admin to accept the work');
  await admin.waitForSelector('button:has-text("Accept")', { timeout: 10000 }); // live update, no reload
  await admin.screenshot({ path: '05-review.png' });
  await admin.click('button:has-text("Accept")'); await admin.waitForSelector('.badge:has-text("Done")');
  await yara.waitForSelector('.badge:has-text("Done")', { timeout: 10000 });
});
await step('history tab', admin, async () => {
  await admin.click('button:has-text("History")'); await admin.waitForSelector('text=ticked'); await admin.waitForSelector('text=changed the status');
  await admin.screenshot({ path: '06-history.png', fullPage: true });
});
await step('tasks list + filters', yara, async () => {
  await yara.goto(B + '/tasks'); await yara.waitForSelector('h1:has-text("Tasks")');
  await yara.selectOption('select.input.w-36', 'done'); await yara.waitForSelector(`td >> text=${TASK}`);
  await yara.screenshot({ path: '07-tasks-list.png' });
});
await step('projects list + dashboard', admin, async () => {
  await admin.goto(B + '/projects'); await admin.waitForSelector(`text=${projName}`); await admin.screenshot({ path: '08-projects.png' });
  await admin.goto(B + '/'); await admin.waitForSelector('h2:has-text("My tasks")'); await admin.screenshot({ path: '09-dashboard.png' });
});
console.log('errors:', errors.length ? errors : 'none');
await browser.close();
await cleanupRun(stamp);
