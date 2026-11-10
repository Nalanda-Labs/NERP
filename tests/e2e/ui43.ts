// Calls from where people work: a colleague's profile (Message and Call), a workgroup, a task
// and a calendar event each have Chat and 📹 Call. The task's chat links back to the task, a
// task call rings the others on it, and pressing Call again joins the same call.
import { chromium } from 'playwright';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
const B = 'http://localhost:8080';
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe, args: ['--use-fake-device-for-media-stream', '--use-fake-ui-for-media-stream'] });
const errors: string[] = [];
const s = Date.now().toString(36);
let n = 0, failed = 0;
const pages: import('playwright').Page[] = [];
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `fail43-${n}-${i}.png`, fullPage: true }); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const mk = async (first: string) => (await admin.post('/users', { email: `${first.toLowerCase()}.${s}@acme.test`, password: 'password123', first_name: first, last_name: `Calls${s}` })).data;
const bobUser = await mk('Bob');
const cyUser = await mk('Cy');
const bobApi = new Client();
await bobApi.post('/auth/login', { email: `bob.${s}@acme.test`, password: 'password123' });
const me = (await admin.get('/auth/me')).data;
const myId = me.id ?? me.user?.id;
const task = (await admin.post('/tasks', { title: `Call me ${s}`, responsible_id: bobUser.id })).data;
const group = (await admin.post('/projects', { name: `Callers ${s}`, kind: 'group' })).data;
const start = new Date(Date.now() + 3600e3);
const event = (await admin.post('/calendar/events', { title: `Standup ${s}`, timezone: 'UTC', start: start.toISOString(), end: new Date(+start + 1800e3).toISOString(), attendees: [bobUser.id] })).data;

const login = async (email: string, password: string) => {
  const p = await (await browser.newContext({ viewport: { width: 1440, height: 900 }, permissions: ['camera', 'microphone'] })).newPage();
  p.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
  p.on('console', (m) => m.type() === 'error' && !/401|400|404|Failed to load resource/.test(m.text()) && errors.push(m.text()));
  pages.push(p);
  await p.goto(B + '/login'); await p.fill('#email', email); await p.fill('#password', password);
  await p.click('button:has-text("Sign in")'); await p.waitForURL(B + '/');
  return p;
};
const ada = await login('admin@acme.test', 'supersecret1');
const bob = await login(`bob.${s}@acme.test`, 'password123');
const leave = async (p: import('playwright').Page) => {
  await p.click('button:has-text("Leave")');
  await p.waitForURL((u) => !u.pathname.startsWith('/call/'));
};
const inCall = async (p: import('playwright').Page) => {
  await p.waitForURL(/\/call\//);
  await p.locator('[data-call-status="in"]').waitFor();
  return new URL(p.url()).pathname;
};

try {
  await step('a profile: Message and Call', async () => {
    await ada.goto(`${B}/employees/${cyUser.id}`);
    await ada.locator('button:has-text("Message")').waitFor();
    await ada.click('[data-call-button]');
    await inCall(ada);
    await ada.locator(`text=Waiting for Cy Calls${s}`).waitFor();
    await leave(ada);
  });

  await step('a task: its chat, linked back to the task', async () => {
    await ada.goto(`${B}/tasks/${task.id}`);
    await ada.click('main button:has-text("Chat")');
    await ada.waitForURL(/\/messenger\?chat=/);
    await ada.locator(`h2:has-text("Call me ${s}")`).waitFor();
    await ada.fill('textarea[aria-label="Message"]', `About the task ${s}`);
    await ada.press('textarea[aria-label="Message"]', 'Enter');
    await ada.locator(`[data-msg="About the task ${s}"]`).waitFor();
    await ada.click('[data-chat-of="task"]');
    await ada.waitForURL(`${B}/tasks/${task.id}`);
  });

  await step('a task call rings the others on it; they join the same call', async () => {
    await ada.goto(`${B}/tasks/${task.id}`);
    await ada.click('main [data-call-button]');
    const path = await inCall(ada);
    await bob.locator('[data-ringing]').waitFor({ timeout: 15_000 });
    await bob.click('[data-ringing] button:has-text("Join")');
    await bob.waitForURL(B + path);
    await bob.locator('[data-call-status="in"]').waitFor();
    await ada.locator(`text=Waiting for`).waitFor({ state: 'detached', timeout: 20_000 });
    await leave(bob);
    // Call from the task again while Ada is still in: the same call.
    await bob.goto(`${B}/tasks/${task.id}`);
    await bob.click('main [data-call-button]');
    if ((await inCall(bob)) !== path) throw new Error('a second call was started');
    await leave(bob);
    await leave(ada);
  });

  await step('a workgroup: Chat, and Call once someone else is in it', async () => {
    await ada.goto(`${B}/projects/${group.id}`);
    await ada.locator('main button:has-text("Chat")').waitFor();
    if (await ada.locator('main [data-call-button]').count()) throw new Error('Call offered with nobody else in the group');
    await admin.put(`/projects/${group.id}/members/${bobUser.id}`, { role: 'member' });
    await ada.reload();
    await ada.click('main [data-call-button]');
    await inCall(ada);
    await leave(ada);
    await ada.locator('[data-chat-of="workgroup"]').waitFor();
  });

  await step('a calendar event: Chat and Call for the guests', async () => {
    await bob.goto(`${B}/calendar?event=${event.id}`);
    await bob.locator('[data-event-chat] button:has-text("Chat")').click();
    await bob.waitForURL(/\/messenger\?chat=/);
    await bob.locator(`h2:has-text("Standup ${s}")`).waitFor();
    await bob.locator('[data-chat-of="event"]').click();
    await bob.locator('[data-event-chat] [data-call-button]').click();
    await inCall(bob);
    await bob.locator(`text=Waiting for`).waitFor();
    await leave(bob);
  });

  await step('no Chat or Call on a task you are not on', async () => {
    const theirs = (await bobApi.post('/tasks', { title: `Private-ish ${s}`, responsible_id: bobUser.id })).data;
    await ada.goto(`${B}/tasks/${theirs.id}`);
    await ada.locator(`h1:has-text("Private-ish ${s}")`).waitFor();
    if (await ada.locator('main [data-call-button]').count()) throw new Error('Call shown to someone not on the task');
  });
} finally {
  await browser.close();
  await cleanupRun(s);
}
console.log(errors.length ? 'console errors:\n' + errors.join('\n') : 'no console errors');
process.exit(failed || errors.length ? 1 : 0);
