// Meetings in the browser: the owner plans a meeting with people and a keeper; a member
// proposes a topic and the presenter writes a report; the keeper starts the meeting, marks
// attendance, records decisions, settles a topic and makes a task; closes it; the minutes
// page shows it all; the owner plans the next meeting with the unfinished topic, whose
// history shows the first meeting.
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
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `fail53-${n}-${i}.png`, fullPage: true }); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
for (const [first, last] of [['Pat', 'Member'], ['Kim', 'Keeper']]) await admin.post('/users', { email: `${first.toLowerCase()}.${s}@acme.test`, password: 'password123', first_name: first, last_name: `${last}${s}` });

const ctx = async (email: string, password: string) => {
  const p = await (await browser.newContext({ viewport: { width: 1440, height: 1000 }, timezoneId: 'UTC', locale: 'en-GB' })).newPage();
  p.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
  p.on('console', (m) => m.type() === 'error' && !/401|400|404|409|Failed to load resource/.test(m.text()) && errors.push(m.text()));
  pages.push(p);
  await p.goto(B + '/login'); await p.fill('#email', email); await p.fill('#password', password);
  await p.click('button:has-text("Sign in")'); await p.waitForURL(B + '/');
  return p;
};
const title = `Sync ${s}`;
let meetingUrl = '';

try {
  const ada = await ctx('admin@acme.test', 'supersecret1');
  const pat = await ctx(`pat.${s}@acme.test`, 'password123');
  const kim = await ctx(`kim.${s}@acme.test`, 'password123');

  await step('the owner plans a meeting with people and a keeper', async () => {
    await ada.click('nav >> text=Meetings');
    await ada.click('button:has-text("New meeting")');
    await ada.fill('#m-title', title);
    await ada.fill('#m-place', 'Room 7');
    await ada.fill('dialog input[placeholder="Add people…"]', `Member${s}`);
    await ada.click(`dialog button:has-text("Pat Member${s}")`);
    await ada.fill('dialog input[placeholder="Choose the keeper…"]', `Keeper${s}`);
    await ada.click(`dialog button:has-text("Kim Keeper${s}")`);
    await ada.click('button[form="new-meeting"]');
    await ada.waitForURL(/\/meetings\/[0-9a-f-]+$/);
    meetingUrl = ada.url();
    await ada.locator('[data-state="prepare"]').waitFor();
    await ada.locator(`[data-person="Kim Keeper${s}"]:has-text("Writes the minutes")`).waitFor();
    await ada.locator(`[data-person="Pat Member${s}"]:has-text("Takes part")`).waitFor();
  });

  await step('a member proposes a topic she presents', async () => {
    await pat.click('nav >> text=Meetings');
    await pat.click(`[data-meeting="${title}"]`);
    await pat.fill('input[aria-label="Topic"]', `Launch date ${s}`);
    await pat.fill('input[aria-label="Minutes for the topic"]', '10');
    await pat.check(`label:has-text("Pat Member${s}") input`);
    await pat.click('button:has-text("Propose")');
    await pat.locator(`[data-topic="Launch date ${s}"]:has-text("proposed by Pat Member${s}")`).waitFor();
  });

  await step('the owner adds a topic for the keeper', async () => {
    await ada.reload();
    await ada.fill('input[aria-label="Topic"]', `Budget ${s}`);
    await ada.check(`label:has-text("Kim Keeper${s}") input`);
    await ada.click('form button:text-is("Add")');
    await ada.locator(`[data-topic="Budget ${s}"]:has-text("Presents: Kim Keeper${s}")`).waitFor();
  });

  await step('the presenter writes her report', async () => {
    await pat.reload();
    await pat.fill(`textarea[aria-label="Your report on Launch date ${s}"]`, 'QA needs two more weeks.');
    await pat.click(`[data-topic="Launch date ${s}"] button:has-text("Save report")`);
    await pat.locator('text=Report saved').waitFor();
    await pat.reload();
    if (await pat.inputValue(`textarea[aria-label="Your report on Launch date ${s}"]`) !== 'QA needs two more weeks.') throw new Error('report not kept');
    await ada.reload();
    await ada.locator(`[data-topic="Launch date ${s}"] [data-report="Pat Member${s}"]:has-text("QA needs two more weeks.")`).waitFor();
    if (await pat.locator('button:has-text("Start the meeting")').count()) throw new Error('members cannot start it');
  });

  await step('the keeper starts it and marks attendance', async () => {
    await kim.goto(meetingUrl);
    await kim.click('button:has-text("Start the meeting")');
    await kim.locator('[data-state="in_progress"]').waitFor();
    await kim.click(`[data-person="Pat Member${s}"] button:has-text("Present")`);
    await kim.click(`[data-person="Kim Keeper${s}"] button:has-text("Present")`);
    await kim.click(`[data-person="Ada Admin"] button:has-text("Absent")`);
    await kim.locator(`[data-person="Ada Admin"] button.bg-red-50:has-text("Absent")`).waitFor();
  });

  await step('decisions, a settled topic and a task', async () => {
    await kim.fill(`textarea[aria-label="Decision on Budget ${s}"]`, 'Budget approved as planned.');
    await kim.locator(`textarea[aria-label="Decision on Budget ${s}"]`).blur();
    await kim.check(`[data-topic="Budget ${s}"] label:has-text("Settled") input`);
    await kim.locator(`[data-topic="Budget ${s}"] [data-status="done"]`).waitFor();
    await kim.fill(`textarea[aria-label="Decision on Launch date ${s}"]`, 'Move the launch by two weeks.');
    await kim.locator(`textarea[aria-label="Decision on Launch date ${s}"]`).blur();
    await kim.click(`[data-topic="Launch date ${s}"] button:has-text("Make a task")`);
    await kim.click('button:has-text("Make the task")');
    await kim.locator(`[data-topic="Launch date ${s}"] [data-task]:has-text("Launch date ${s}")`).waitFor();
  });

  await step('notes, and closing sends the minutes', async () => {
    await kim.fill('textarea[aria-label="Notes for the minutes"]', 'Good meeting.');
    await kim.click('button:has-text("Save notes")');
    kim.once('dialog', (d) => d.accept());
    await kim.click('button:has-text("Close and send the minutes")');
    await kim.locator('[data-state="closed"]').waitFor();
  });

  await step('the minutes page', async () => {
    await pat.goto(meetingUrl + '/minutes');
    await pat.locator(`[data-attendance]:has-text("Pat Member${s}")`).waitFor();
    await pat.locator(`[data-minutes-topic="Budget ${s}"]:has-text("Budget approved as planned.")`).waitFor();
    await pat.locator(`[data-minutes-topic="Launch date ${s}"]:has-text("QA needs two more weeks.")`).waitFor();
    await pat.locator(`[data-minutes-topic="Launch date ${s}"]:has-text("Task #")`).waitFor();
    await pat.locator('[data-minutes-notes]:has-text("Good meeting.")').waitFor();
  });

  await step('the next meeting carries the unfinished topic, with its history', async () => {
    await ada.goto(meetingUrl);
    await ada.click('button:has-text("Plan the next meeting")');
    await ada.click('dialog button:has-text("Plan it")');
    await ada.waitForURL((u) => u.pathname.startsWith('/meetings/') && u.toString() !== meetingUrl);
    await ada.locator('[data-previous]').waitFor();
    await ada.locator(`[data-topic="Launch date ${s}"]`).waitFor();
    if (await ada.locator(`[data-topic="Budget ${s}"]`).count()) throw new Error('the settled topic was carried');
    await ada.click(`[data-topic="Launch date ${s}"] button:has-text("History (1)")`);
    await ada.locator(`[data-history="${title}"]:has-text("Move the launch by two weeks.")`).waitFor();
  });
} finally {
  await browser.close();
  await cleanupRun(s);
}
console.log(errors.length ? 'console errors:\n' + errors.join('\n') : 'no console errors');
process.exit(failed || errors.length ? 1 : 0);
