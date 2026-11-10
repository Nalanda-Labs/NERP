// Check-in in the browser: check in from HR → Check-in (working from home, telling a chat, with
// where I am, starting the work day); the top bar shows it; a colleague's check-in appears live
// in the department's day; the month grid; "not working after all"; my check-ins; the reminder
// in the top bar and muting it.
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
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `fail50-${n}-${i}.png`, fullPage: true }); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const user = async (first: string) => {
  const u = (await admin.post('/users', { email: `${first.toLowerCase()}.${s}@acme.test`, password: 'password123', first_name: first, last_name: 'Checkin' })).data;
  const c = new Client();
  await c.post('/auth/login', { email: `${first.toLowerCase()}.${s}@acme.test`, password: 'password123' });
  c.id = u.id;
  return c;
};
const before = (await admin.get('/checkin/settings')).data;
await admin.patch('/checkin/settings', { enabled: true, geo_enabled: true });
const ann = await user('Ann');
const bob = await user('Bob');
const cat = await user('Cat');
const team = (await admin.post('/departments', { name: `Crew ${s}`, head_user_id: ann.id })).data;
await admin.post(`/departments/${team.id}/members`, { user_id: ann.id });
await admin.post(`/departments/${team.id}/members`, { user_id: bob.id });
const chat = (await ann.post('/im/chats', { title: `Standup ${s}`, members: [bob.id, cat.id] })).data.id;

const ctx = async (email: string) => {
  const c = await browser.newContext({ viewport: { width: 1440, height: 1000 }, timezoneId: 'UTC', locale: 'en-GB', permissions: ['geolocation'], geolocation: { latitude: 48.85837, longitude: 2.294481 } });
  const p = await c.newPage();
  p.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
  p.on('console', (m) => m.type() === 'error' && !/401|400|404|409|Failed to load resource/.test(m.text()) && errors.push(m.text()));
  pages.push(p);
  await p.goto(B + '/login'); await p.fill('#email', email); await p.fill('#password', 'password123');
  await p.click('button:has-text("Sign in")'); await p.waitForURL(B + '/');
  return p;
};

try {
  const a = await ctx(`ann.${s}@acme.test`);

  await step('check in: from home, telling the standup chat, with where I am', async () => {
    await a.goto(B + '/hr/checkin');
    await a.click('[data-today] button:has-text("Check in")');
    await a.click('dialog[open] [role=radio]:has-text("From home")');
    await a.check('dialog[open] label:has-text("Tell a chat") input');
    const opts = await a.locator('#ck-chat option').allTextContents();
    await a.selectOption('#ck-chat', { label: opts.find((o) => o.includes(`Standup ${s}`))! });
    await a.fill('#ck-msg', 'Writing the quarterly report');
    await a.check('dialog[open] label:has-text("Share where I am") input');
    await a.locator('[data-geo="found"]').waitFor();
    await a.fill('#ck-addr', 'Champ de Mars, Paris');
    await a.check('dialog[open] label:has-text("Start my work day too") input');
    await a.click('dialog[open] button:has-text("Check in")');
    await a.locator('[data-my-summary]:has-text("Working from home")').waitFor();
  });

  await step('the top bar shows it, and the work day started', async () => {
    await a.locator('header [data-checkin="working"]:has-text("From home")').or(a.locator('[data-checkin="working"]:has-text("From home")')).first().waitFor();
    await a.locator('[data-workday="working"]').waitFor();
  });

  await step('the chat got it', async () => {
    const msgs = (await bob.get(`/im/chats/${chat}/messages`)).data.items;
    const m = msgs.find((x: any) => x.body.startsWith('✅ Working from home today'));
    if (!m || !m.body.includes('Writing the quarterly report') || !m.body.includes('📍 Champ de Mars, Paris')) throw new Error(JSON.stringify(msgs.map((x: any) => x.body)));
  });

  await step('the department\'s day: me, working, with the place', async () => {
    await a.locator(`[data-group="working"] [data-person="Ann Checkin"]:has-text("Working from home")`).waitFor();
    await a.locator(`[data-group="working"] [data-person="Ann Checkin"] a:has-text("Champ de Mars, Paris")`).waitFor();
    await a.locator(`[data-group="none"] [data-person="Bob Checkin"]`).waitFor();
  });

  await step('a colleague\'s check-in appears live', async () => {
    await bob.post('/checkin', { status: 'not_working', reason: 'vacation' });
    await a.locator(`[data-group="not_working"] [data-person="Bob Checkin"]:has-text("Not working: Vacation")`).waitFor();
    await a.locator('[data-total="not_working"]:text-is("1")').waitFor();
  });

  await step('the month grid', async () => {
    await a.click('[role=tab]:has-text("Month")');
    const today = new Date().toISOString().slice(0, 10);
    await a.locator(`tr[data-person="Ann Checkin"] [data-cell="${today}"]`).waitFor();
    await a.locator(`tr[data-person="Ann Checkin"] [data-working]:text-is("1")`).waitFor();
  });

  await step('not working after all, from the top bar', async () => {
    await a.click('[data-checkin="working"]');
    await a.click('dialog[open] button:has-text("Not working after all")');
    await a.click('dialog[open] [role=radio]:has-text("Other")');
    await a.fill('#ck-cr', 'Boiler broke');
    await a.click('dialog[open] button:has-text("Not working today")');
    await a.locator('dialog[open] [data-summary]:has-text("Not working after all: Boiler broke")').waitFor();
    await a.click('dialog[open] button:has-text("Close")');
    await a.locator('[data-checkin="cancelled"]').first().waitFor();
    await a.locator('[data-my-summary]:has-text("Not working after all: Boiler broke")').waitFor();
  });

  await step('my check-ins', async () => {
    await a.click('[role=tab]:has-text("My check-ins")');
    await a.locator(`[data-mine]:has-text("Boiler broke")`).waitFor();
  });

  await step('Cat, in the chat, is reminded; muting it for today', async () => {
    const due = (await cat.get('/checkin/today')).data.working_day;
    const c = await ctx(`cat.${s}@acme.test`);
    await c.locator(`[data-checkin="${due ? 'due' : 'none'}"]`).waitFor();
    await c.click('[data-checkin]');
    if (due) {
      await c.click('dialog[open] summary:has-text("Remind me later")');
      await c.click('dialog[open] button:has-text("Not today")');
      await c.locator('[data-checkin="none"]').waitFor();
    } else {
      await c.click('dialog[open] button:has-text("Cancel")');
    }
  });
} finally {
  await browser.close();
  await admin.patch('/checkin/settings', { enabled: before.enabled, geo_enabled: before.geo_enabled });
  await cleanupRun(s);
}
console.log(errors.length ? 'console errors:\n' + errors.join('\n') : 'no console errors');
process.exit(failed || errors.length ? 1 : 0);
