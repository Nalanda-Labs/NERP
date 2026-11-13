import { chromium } from 'playwright';
import { cleanupRun } from './cleanup.ts';
const B = 'http://localhost:8080';
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: any[] = [];
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const c = await browser.newContext({ viewport: { width: 1440, height: 900 } });
const page = await c.newPage();
page.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
page.on('console', (m) => m.type() === 'error' && !/401|Failed to load resource/.test(m.text()) && errors.push(m.text()));
let n = 0;
const step = async (name, fn) => { try { await fn(); console.log('ok  ', name); } catch (e: any) { console.log('FAIL', name, e.message.split('\n')[0]); await page.screenshot({ path: `fail-${++n}.png`, fullPage: true }); } };
const api = async (method: string, path: string, body?: any) => {
  const r = await fetch(B + '/api' + path, { method, headers: { 'content-type': 'application/json', authorization: `Bearer ${tok}` }, body: body && JSON.stringify(body) });
  return r.json();
};
let tok = (await (await fetch(B + '/api/auth/login', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ email: 'admin@acme.test', password: 'supersecret1' }) })).json()).access_token;
const stamp = Date.now().toString(36);
const D = 864e5, today = new Date(); today.setHours(9, 0, 0, 0);
const at = (d) => new Date(today.getTime() + d * D).toISOString();
const proj = await api('POST', '/projects', { name: `Roadmap ${stamp}` });
const A = await api('POST', '/tasks', { title: `Research ${stamp}`, project_id: proj.id, start_date: at(0), deadline: at(2) });
const Bt = await api('POST', '/tasks', { title: `Prototype ${stamp}`, project_id: proj.id, start_date: at(2), deadline: at(5) });
const C = await api('POST', '/tasks', { title: `User testing ${stamp}`, project_id: proj.id, start_date: at(6), deadline: at(8) });
await api('POST', `/tasks/${Bt.id}/dependencies`, { depends_on_id: A.id });

await page.goto(B + '/login'); await page.fill('#email', 'admin@acme.test'); await page.fill('#password', 'supersecret1');
await page.click('button:has-text("Sign in")'); await page.waitForURL(B + '/');

await step('project board uses its columns', async () => {
  await page.goto(`${B}/projects/${proj.id}`);
  for (const col of ['To do', 'In progress', 'Review', 'Done']) await page.waitForSelector(`section h3:text-is("${col}")`);
});
await step('add a custom column', async () => {
  await page.click('button:has-text("Edit columns")');
  await page.fill('dialog input[placeholder="New column name"]', 'Design');
  await page.click('dialog button:has-text("Add column")');
  await page.waitForFunction(() => [...document.querySelectorAll<HTMLInputElement>('dialog input')].some((i) => i.value === 'Design'));
  await page.screenshot({ path: '01-columns.png' });
  await page.click('dialog button:has-text("Done")');
  await page.waitForSelector('section h3:text-is("Design")');
});
await step('drag card into custom column keeps status', async () => {
  await page.locator('a[data-card]:has-text("User testing")').dragTo(page.locator('section:has(h3:text-is("Design")) [role=list]'));
  await page.waitForSelector('section:has(h3:text-is("Design")) >> text=User testing');
  await sleep(400);
  const t = await api('GET', `/tasks/${C.id}`);
  if (t.status !== 'new') throw new Error('status changed: ' + t.status);
});
await step('drag to Done completes', async () => {
  await page.locator('a[data-card]:has-text("Research")').dragTo(page.locator('section:has(h3:text-is("Done")) [role=list]'));
  await sleep(600);
  const t = await api('GET', `/tasks/${A.id}`);
  if (t.status !== 'done') throw new Error('status ' + t.status);
  await page.screenshot({ path: '02-board.png' });
});
await step('gantt shows bars and dependency arrow', async () => {
  await api('POST', `/tasks/${A.id}/status`, { status: 'in_progress' });
  await page.click('button:has-text("Gantt")');
  await page.waitForSelector(`[data-bar="${A.id}"]`); await page.waitForSelector(`[data-bar="${Bt.id}"]`);
  // Dependencies load after the bars.
  await page.locator('svg path[marker-end]').first().waitFor({ timeout: 5000 }).catch(() => { throw new Error('no arrows'); });
  await page.screenshot({ path: '03-gantt.png' });
});
await step('drag bar later -> dependant shifts', async () => {
  // Let the live refresh after the status change land first, so the bar is where it stays.
  await page.waitForLoadState('networkidle');
  await sleep(500);
  const bar = page.locator(`[data-bar="${A.id}"]`);
  const box = (await bar.boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width / 2 + 36 * 2, box.y + box.height / 2, { steps: 8 }); // +2 days at 36px/day
  await page.mouse.up();
  await sleep(1200);
  const a = await api('GET', `/tasks/${A.id}`), b = await api('GET', `/tasks/${Bt.id}`);
  const shiftA = Math.round((+new Date(a.deadline) - +new Date(at(2))) / D), startB = Math.round((+new Date(b.start_date) - +new Date(at(2))) / D);
  if (shiftA !== 2 || startB !== 2) throw new Error(`A moved ${shiftA} days, B start moved ${startB}`);
  await page.screenshot({ path: '04-gantt-moved.png' });
});
await step('resize bar end changes deadline', async () => {
  const bar = page.locator(`[data-bar="${C.id}"]`);
  const box = (await bar.boundingBox())!;
  await page.mouse.move(box.x + box.width - 3, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width - 3 + 36, box.y + box.height / 2, { steps: 6 });
  await page.mouse.up();
  await sleep(1000);
  const t = await api('GET', `/tasks/${C.id}`);
  if (Math.round((+new Date(t.deadline) - +new Date(at(8))) / D) !== 1) throw new Error('deadline ' + t.deadline);
});
await step('task page: add dependency with picker', async () => {
  await page.goto(`${B}/tasks/${C.id}`);
  await page.waitForSelector('h2:has-text("Dependencies")');
  await page.fill('input[placeholder="This task waits for…"]', `Prototype ${stamp}`);
  await page.click('ul >> button:has-text("Prototype")');
  await page.waitForSelector('section:has(h2:has-text("Dependencies")) a:has-text("Prototype")');
  await page.waitForSelector('text=Waiting for 1 unfinished task');
  await page.screenshot({ path: '05-deps.png', fullPage: true });
});
await step('save as template and make it recurring', async () => {
  await page.click('button[aria-label="Save as template"]');
  await page.waitForURL(/\/tasks\/templates\/[0-9a-f-]{36}$/);
  await page.selectOption('#rec-freq', 'weekly');
  // Weekly starts with today's weekday chosen; add another one (clicking today's would unselect it).
  await page.click(`button:text-is("${new Date().getDay() === 5 ? 'Thu' : 'Fri'}")`);
  await page.fill('#rec-time', '10:30');
  await page.screenshot({ path: '06-template.png', fullPage: true });
  await page.click('button:has-text("Save template")');
  await page.waitForURL(B + '/tasks/templates');
  await page.waitForSelector('text=Every week on');
  await page.screenshot({ path: '07-templates.png' });
});
await step('create task from template', async () => {
  await page.click('li:has-text("User testing") button:has-text("Create task now")');
  await page.waitForURL(/\/tasks\/[0-9a-f-]{36}$/);
  await page.waitForSelector('text=Recurring: Every week on');
});
await step('my tasks gantt layout', async () => {
  await page.goto(`${B}/tasks?layout=gantt&view=created`);
  await page.waitForSelector('[data-bar]');
});
console.log('errors:', errors.length ? errors : 'none');
await browser.close();
await cleanupRun(stamp);
