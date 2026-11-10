// Task depth, part two, in the browser: a board rule set up in the automation editor runs
// when a card is dragged into its column; a project is copied; a flow is made, gets a task
// from someone outside the group, and its team member takes it; the efficiency report.
import { chromium } from 'playwright';
import { Client, waitFor } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
const B = 'http://localhost:8080';
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: string[] = [];
const s = Date.now().toString(36);
let n = 0, failed = 0;
const pages: import('playwright').Page[] = [];
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `fail48-${n}-${i}.png`, fullPage: true }); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const bob = (await admin.post('/users', { email: `bob.${s}@acme.test`, password: 'password123', first_name: 'Bob', last_name: `Board${s}` })).data;
await admin.post('/users', { email: `eve.${s}@acme.test`, password: 'password123', first_name: 'Eve', last_name: `Outside${s}` });
const proj = (await admin.post('/projects', { name: `Print shop ${s}`, member_ids: [bob.id] })).data;
const task = (await admin.post('/tasks', { title: `Business cards ${s}`, project_id: proj.id })).data;

const login = async (email: string, password: string) => {
  const p = await (await browser.newContext({ viewport: { width: 1440, height: 1000 } })).newPage();
  p.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
  p.on('console', (m) => m.type() === 'error' && !/401|400|404|Failed to load resource/.test(m.text()) && errors.push(m.text()));
  pages.push(p);
  await p.goto(B + '/login'); await p.fill('#email', email); await p.fill('#password', password);
  await p.click('button:has-text("Sign in")'); await p.waitForURL(B + '/');
  return p;
};
const ada = await login('admin@acme.test', 'supersecret1');

try {
  await step('a rule in the automation editor', async () => {
    await ada.goto(`${B}/projects/${proj.id}?tab=board`);
    await ada.click('[data-automation]');
    const col = ada.locator('[data-automation-stage="In progress"]');
    await col.locator('button:has-text("Add rule")').click();
    await col.locator('select[aria-label="Rule"]').selectOption('set_priority');
    await col.locator('select[aria-label="Priority"]').selectOption('high');
    await col.locator('button:text-is("Add")').click();
    await col.locator('[data-rule="set_priority"]').waitFor();
    await ada.selectOption('select[aria-label="Trigger: Someone comments"]', { label: 'Review' });
    await ada.click('dialog[open] button:has-text("Close")');
  });

  await step('dragging a card into the column runs it', async () => {
    await ada.locator(`[data-card]:has-text("Business cards ${s}")`).dragTo(ada.locator('[data-column]').nth(1));
    await waitFor(async () => (await admin.get(`/tasks/${task.id}`)).data.priority === 'high');
    await ada.reload();
    await ada.locator(`[data-card]:has-text("Business cards ${s}") >> text=▲`).waitFor();
  });

  await step('copy the project', async () => {
    await ada.click('button:has-text("Settings")');
    await ada.click('button:has-text("Copy as a new project")');
    await ada.fill('#cp-name', `Print shop 2 ${s}`);
    await ada.click('button:text-is("Copy")');
    await ada.locator(`h1:has-text("Print shop 2 ${s}")`).waitFor();
    await ada.locator(`[data-card]:has-text("Business cards ${s}")`).waitFor();
  });

  await step('a flow; someone outside sends it a task; the team takes it', async () => {
    await ada.goto(`${B}/flows?project_id=${proj.id}`);
    await ada.click('button:has-text("New flow")');
    await ada.selectOption('#fl-g', { label: `Print shop ${s}` });
    await ada.fill('#fl-n', `Print requests ${s}`);
    await ada.selectOption('#fl-dist', 'self');
    await ada.locator('dialog[open] input[placeholder^="Who works"]').fill('Bob');
    await ada.locator(`dialog[open] >> text=Bob Board${s}`).first().click();
    await ada.fill('#fl-h', '4');
    await ada.click('dialog[open] button:has-text("Create")');
    await ada.locator(`h1:has-text("Print requests ${s}")`).waitFor();
    const flowUrl = ada.url();
    const eve = await login(`eve.${s}@acme.test`, 'password123');
    await eve.goto(flowUrl);
    await eve.click('button:has-text("Send a task")');
    await eve.fill('#fs-t', `Poster A2 ${s}`);
    await eve.click('dialog[open] button:has-text("Send")');
    await eve.locator('text=Send this flow a task').waitFor();
    const bobPage = await login(`bob.${s}@acme.test`, 'password123');
    await bobPage.goto(flowUrl);
    const card = bobPage.locator(`[data-flow-task="Poster A2 ${s}"]`);
    await card.locator('button:has-text("Take it")').click();
    await bobPage.waitForURL(/\/tasks\//);
    await ada.reload();
    await ada.locator(`[data-member="Bob Board${s}"]:has-text("1 open")`).waitFor();
  });

  await step('the efficiency report', async () => {
    await ada.goto(`${B}/tasks/reports`);
    await ada.selectOption('select[aria-label="Whose"]', 'all');
    await ada.locator(`[data-person="Bob Board${s}"]`).waitFor();
  });
} finally {
  await browser.close();
  await cleanupRun(s);
}
console.log(errors.length ? 'console errors:\n' + errors.join('\n') : 'no console errors');
process.exit(failed || errors.length ? 1 : 0);
