// HR in the browser: HR sets a deputy and makes reports weekly; an employee plans the day in
// My day, starts work, ticks items off, and sends the day's report filled in from the plan;
// the deputy marks it; the deputy puts the employee on a shift in the rota and sets up a
// shift... (HR only).
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
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `fail58-${n}-${i}.png`, fullPage: true }); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const ids: Record<string, string> = {};
for (const first of ['Gil', 'Rae']) ids[first] = (await admin.post('/users', { email: `${first.toLowerCase()}.${s}@acme.test`, password: 'password123', first_name: first, last_name: `Staff${s}` })).data.id;
const dept = (await admin.post('/departments', { name: `Ops ${s}` })).data;
await admin.post(`/departments/${dept.id}/members`, { user_id: ids.Gil });
const shift = (await admin.post('/hr/shifts', { name: `Day ${s}`, start_min: 480, end_min: 960 })).data.items.find((x: any) => x.name === `Day ${s}`);

const ctx = async (email: string, password = 'password123') => {
  const p = await (await browser.newContext({ viewport: { width: 1440, height: 1000 }, timezoneId: 'UTC', locale: 'en-GB' })).newPage();
  p.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
  p.on('console', (m) => m.type() === 'error' && !/401|400|404|409|Failed to load resource/.test(m.text()) && errors.push(m.text()));
  pages.push(p);
  await p.goto(B + '/login'); await p.fill('#email', email); await p.fill('#password', password);
  await p.click('button:has-text("Sign in")'); await p.waitForURL(B + '/');
  return p;
};

try {
  const ada = await ctx('admin@acme.test', 'supersecret1');
  const gil = await ctx(`gil.${s}@acme.test`);
  const rae = await ctx(`rae.${s}@acme.test`);

  await step('HR makes Rae the deputy of Ops', async () => {
    await ada.goto(B + '/structure');
    await ada.click(`text=Ops ${s}`);
    await ada.click('main button:has-text("Edit")');
    await ada.locator('dialog input[placeholder="Search employees…"]').nth(1).fill(`Rae Staff${s}`);
    await ada.click(`dialog button:has-text("Rae Staff${s}")`);
    await ada.click('dialog button:has-text("Save")');
    await ada.locator(`[data-deputy]:has-text("Rae Staff${s}")`).waitFor();
  });

  await step('Gil plans the day, works, and sends the day\'s report', async () => {
    await gil.goto(B + '/hr/my-day');
    await gil.click('button:has-text("Start the day")');
    await gil.locator('[data-day-state="working"]').waitFor();
    for (const t of [`Fix the printer ${s}`, `Order paper ${s}`]) {
      await gil.fill('input[aria-label="Add to my plan"]', t);
      await gil.click('[aria-label="My plan"] button:has-text("Add")');
      await gil.locator(`[data-plan] >> text=${t}`).waitFor();
    }
    await gil.check(`input[aria-label="Done: Fix the printer ${s}"]`);
    await gil.locator(`[data-plan] .line-through:has-text("Fix the printer ${s}")`).waitFor();
    await gil.click('button:has-text("Fill in from my plan")');
    if (!(await gil.inputValue('textarea[aria-label="What I did"]')).includes(`Fix the printer ${s}`)) throw new Error('not filled');
    if (!(await gil.inputValue('textarea[aria-label="Plans"]')).includes(`Order paper ${s}`)) throw new Error('plans not filled');
    await gil.click('[aria-label="Today\'s report"] button:has-text("Send")');
    await gil.locator('[data-report="sent"]').waitFor();
  });

  await step('the deputy reads and marks it', async () => {
    await rae.goto(B + '/hr/reports');
    await rae.click('[role="tab"]:has-text("To read")');
    await rae.click(`[data-report="Gil Staff${s}"]`);
    await rae.locator(`[data-body]:has-text("Fix the printer ${s}")`).waitFor();
    await rae.fill('textarea[aria-label="Comment"]', 'Thanks');
    await rae.click('button:has-text("👍 Good")');
    await rae.locator('[data-mark="good"]').waitFor();
    await gil.goto(B + '/hr/reports');
    await gil.locator('text=👍 Good').waitFor();
  });

  await step('the deputy puts Gil on a shift; Gil sees it in My day', async () => {
    await rae.goto(B + '/hr/rota');
    const today = new Date().toISOString().slice(0, 10);
    await rae.selectOption(`select[aria-label="Gil Staff${s} on ${today}"]`, { label: `Day ${s}` });
    await rae.waitForTimeout(500);
    await gil.goto(B + '/hr/my-day');
    await gil.locator(`[data-shift]:has-text("Day ${s}")`).waitFor();
  });

  await step('only HR sets up shifts', async () => {
    if (await rae.locator('button:has-text("Add shift")').count()) throw new Error('deputy can add shifts');
    await ada.goto(B + '/hr/rota');
    await ada.fill('input[aria-label="Shift name"]', `Late ${s}`);
    await ada.click('button:has-text("Add shift")');
    await ada.locator(`[aria-label="Shifts"] >> text=Late ${s}`).waitFor();
  });

  await step('no browser errors', async () => { if (errors.length) throw new Error(errors.join(' | ')); });
} finally {
  await browser.close();
  await admin.del(`/hr/shifts/${shift.id}`).catch(() => {});
  for (const x of (await admin.get('/hr/shifts')).data.items.filter((x: any) => x.name.endsWith(s))) await admin.del(`/hr/shifts/${x.id}`);
  await cleanupRun(s);
}
console.log(failed ? `\n${failed} failed` : '\nall passed');
process.exit(failed ? 1 : 0);
