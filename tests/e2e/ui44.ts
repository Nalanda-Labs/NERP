// Scrum in the browser: create a Scrum team, fill the backlog, estimate, plan a sprint by
// dragging and by "Move to", start it, finish a task on the sprint board through the
// definition of done, complete the sprint, and read the burndown, velocity and history.
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
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); await p.screenshot({ path: `fail44-${++n}.png`, fullPage: true }); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const dev = (await admin.post('/users', { email: `dev.${s}@acme.test`, password: 'password123', first_name: 'Dev', last_name: `Scrum${s}` })).data;

const p = await (await browser.newContext({ viewport: { width: 1440, height: 1000 } })).newPage();
p.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
p.on('console', (m) => m.type() === 'error' && !/401|400|404|Failed to load resource/.test(m.text()) && errors.push(m.text()));
await p.goto(B + '/login'); await p.fill('#email', 'admin@acme.test'); await p.fill('#password', 'supersecret1');
await p.click('button:has-text("Sign in")'); await p.waitForURL(B + '/');

let teamId = '';
const list = (name: string) => p.locator(`section[data-list="${name}"]`);
const row = (title: string) => p.locator(`[data-task="${title} ${s}"]`);

try {
  await step('create a Scrum team', async () => {
    await p.goto(B + '/projects');
    await p.click('button:has-text("New Scrum team")');
    await p.fill('#p-name', `Rockets ${s}`);
    await p.click('button:has-text("Create Scrum team")');
    await p.waitForURL(/\/projects\/[0-9a-f-]+$/);
    teamId = p.url().split('/').pop()!;
    await p.locator('[data-group-meta]:has-text("Scrum team")').waitFor();
    await p.locator('[role=tab]:has-text("Planning")[aria-selected=true]').waitFor();
    await admin.put(`/projects/${teamId}/members/${dev.id}`, { role: 'member' });
  });

  await step('fill the backlog and estimate', async () => {
    for (const t of ['Launch', 'Orbit', 'Land']) {
      await p.fill('input[aria-label="Add a task to the backlog"]', `${t} ${s}`);
      await p.press('input[aria-label="Add a task to the backlog"]', 'Enter');
      await row(t).waitFor();
    }
    await row('Launch').locator('select[aria-label="Story points"]').selectOption('5');
    await row('Orbit').locator('select[aria-label="Story points"]').selectOption('3');
    await row('Land').locator('select[aria-label="Story points"]').selectOption('8');
    await list('Backlog').locator('[data-list-points]:has-text("3 tasks · 16 points")').waitFor();
  });

  await step('an epic', async () => {
    await p.click('button[aria-label="New epic"]');
    await p.fill('#ep-name', `Mars ${s}`);
    await p.click('button:has-text("Save")');
    await p.locator(`[data-epic="Mars ${s}"]`).waitFor();
    await row('Land').locator('select[aria-label="Epic"]').selectOption({ label: `Mars ${s}` });
    await row('Land').locator(`.badge:has-text("Mars ${s}")`).waitFor();
  });

  await step('plan a sprint: drag one task in, move another', async () => {
    await p.click('button:has-text("Plan a sprint")');
    await list('Sprint 1').waitFor();
    await row('Launch').dragTo(list('Sprint 1').locator('ul'));
    await list('Sprint 1').locator(`[data-task="Launch ${s}"]`).waitFor();
    await row('Orbit').locator('select[aria-label="Move to"]').selectOption({ label: 'Sprint 1' });
    await list('Sprint 1').locator('[data-list-points]:has-text("2 tasks · 8 points")').waitFor();
    await list('Backlog').locator('[data-list-points]:has-text("1 task · 8 points")').waitFor();
  });

  await step('definition of done and team meetings in the Scrum settings', async () => {
    await p.click('button:has-text("Scrum")');
    await p.fill('#sc-dod', 'Reviewed\nTested');
    await p.click('button:has-text("Plan team meetings")');
    await p.click('[role=radio]:has-text("Daily stand-up")');
    await p.click('button:has-text("Add to calendar")');
    await p.locator('text=Daily stand-up is on the team\'s calendar').waitFor();
    await p.click('button[form="scrum-settings"]');
    await p.locator('#sc-dod').waitFor({ state: 'hidden' });
    const r = await admin.get(`/projects/${teamId}/dod`);
    if (r.data.length !== 2) throw new Error('dod not saved: ' + JSON.stringify(r.data));
  });

  await step('start the sprint', async () => {
    await list('Sprint 1').locator('button:has-text("Start sprint")').click();
    await p.fill('#ss-goal', 'Reach orbit');
    await p.click('dialog[open] button:has-text("Start")');
    await list('Sprint 1').locator('.badge:has-text("Active")').waitFor();
  });

  await step('the sprint board: done only after the definition of done', async () => {
    await p.click('[role=tab]:has-text("Active sprint")');
    await p.locator('[data-active-sprint="Sprint 1"]').waitFor();
    await p.locator('text=Goal: Reach orbit').waitFor();
    const card = p.locator(`[data-card]:has-text("Launch ${s}")`);
    await card.locator('.badge:has-text("5 pts")').waitFor();
    const doneCol = p.locator('[data-column]').last();
    await card.dragTo(doneCol);
    await p.locator('dialog[open]:has-text("Definition of done")').waitFor();
    await p.check('dialog[open] label:has-text("Reviewed") input');
    await p.check('dialog[open] label:has-text("Tested") input');
    await p.click('dialog[open] button:has-text("Done")');
    await p.locator('[data-sprint-progress]:has-text("5 / 8 points")').waitFor();
  });

  await step('burndown', async () => {
    await p.click('[role=tab]:has-text("Reports")');
    await p.locator('[data-burn-summary]:has-text("Committed 8")').waitFor();
    await p.locator('svg[aria-label="Burndown chart"]').hover();
  });

  await step('complete the sprint; unfinished work returns to the backlog', async () => {
    await p.click('[role=tab]:has-text("Planning")');
    await list('Sprint 1').locator('button:has-text("Complete sprint")').click();
    await p.locator('dialog[open]:has-text("1 of 2 tasks done")').waitFor();
    await p.click('dialog[open] button:has-text("Complete")');
    await list('Sprint 1').waitFor({ state: 'detached' });
    await list('Backlog').locator(`[data-task="Orbit ${s}"]`).waitFor();
  });

  await step('velocity and sprint history', async () => {
    await p.click('[role=tab]:has-text("Reports")');
    await p.locator('[data-velocity-average]:has-text("5")').waitFor();
    await p.locator('[data-history="Sprint 1"]:has-text("1 / 2")').waitFor();
    await p.locator('svg[aria-label="Burndown chart"]').hover();
    await p.screenshot({ path: 'scrum-reports.png', fullPage: true });
  });

  await step('the meeting is on the team calendar', async () => {
    await p.click('[role=tab]:has-text("Calendar")');
    await p.locator('text=Daily stand-up').first().waitFor();
  });

  await step('the groups list filters Scrum teams', async () => {
    await p.goto(B + '/projects');
    await p.selectOption('select[aria-label="Kind"]', 'scrum');
    await p.locator(`a:has-text("Rockets ${s}"):has-text("Scrum team")`).waitFor();
  });
} finally {
  await browser.close();
  await cleanupRun(s);
}
console.log(errors.length ? 'console errors:\n' + errors.join('\n') : 'no console errors');
process.exit(failed || errors.length ? 1 : 0);
