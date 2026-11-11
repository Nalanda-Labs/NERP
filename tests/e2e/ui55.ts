// Whiteboards in the browser: a board from the Kanban template; drawing a shape that saves;
// a colleague with edit rights sees it arrive live and adds one that shows up for the owner;
// selected text made into a task; a view link opened by someone outside (read-only, live);
// a board inside a task.
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
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `fail55-${n}-${i}.png`, fullPage: true }); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const ids: Record<string, string> = {};
for (const first of ['Uma', 'Vic']) ids[first] = (await admin.post('/users', { email: `${first.toLowerCase()}.${s}@acme.test`, password: 'password123', first_name: first, last_name: `Draw${s}` })).data.id;
const api = new Client();
await api.post('/auth/login', { email: `uma.${s}@acme.test`, password: 'password123' });

const newPage = async () => {
  const p = await (await browser.newContext({ viewport: { width: 1440, height: 1000 }, timezoneId: 'UTC', locale: 'en-GB' })).newPage();
  p.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
  // The app's permissions policy refuses Excalidraw's unload handler; that's fine.
  p.on('console', (m) => m.type() === 'error' && !/401|400|404|409|Failed to load resource|unload is not allowed/.test(m.text()) && errors.push(m.text()));
  pages.push(p);
  return p;
};
const ctx = async (email: string) => {
  const p = await newPage();
  await p.goto(B + '/login'); await p.fill('#email', email); await p.fill('#password', 'password123');
  await p.click('button:has-text("Sign in")'); await p.waitForURL(B + '/');
  return p;
};
const scene = async (id: string) => (await api.get(`/boards/${id}/scene`)).data.scene.elements.filter((e: any) => !e.isDeleted);
const draw = async (p: import('playwright').Page, x: number, y: number) => {
  await p.locator('[data-testid="toolbar-rectangle"]').locator('..').click();
  await p.mouse.move(x, y);
  await p.mouse.down();
  await p.mouse.move(x + 120, y + 80, { steps: 5 });
  await p.mouse.up();
  await p.keyboard.press('Escape');
};
let boardId = '';

try {
  const uma = await ctx(`uma.${s}@acme.test`);
  const vic = await ctx(`vic.${s}@acme.test`);

  await step('a board from the Kanban template', async () => {
    await uma.goto(B + '/boards');
    await uma.click('button:has-text("New board")');
    await uma.fill('#nb-title', `Sprint wall ${s}`);
    await uma.check('input[value="kanban"]');
    await uma.click('button[form="nb-form"]');
    await uma.waitForURL(/\/boards\/[0-9a-f-]+$/);
    boardId = uma.url().split('/').pop()!;
    await uma.locator('[data-excalidraw] canvas').first().waitFor();
    if ((await scene(boardId)).length < 6) throw new Error('template not drawn');
  });

  await step('drawing a shape saves it', async () => {
    const before = (await scene(boardId)).length;
    await draw(uma, 1150, 600);
    await uma.locator('[data-saving]:has-text("Saved")').waitFor();
    for (let i = 0; i < 20 && (await scene(boardId)).length <= before; i++) await uma.waitForTimeout(250);
    if ((await scene(boardId)).length !== before + 1) throw new Error('shape not saved');
  });

  await step('a colleague with edit rights draws too, and each sees the other live', async () => {
    await api.put(`/boards/${boardId}/access`, { grants: [{ kind: 'user', id: ids.Vic, level: 'edit' }] });
    await vic.goto(`${B}/boards/${boardId}`);
    await vic.locator('[data-excalidraw] canvas').first().waitFor();
    await vic.locator(`[data-viewers]:has-text("Uma Draw${s}")`).waitFor({ timeout: 25000 });
    const before = (await scene(boardId)).length;
    // Uma's page asks for the newer drawing as soon as the live note arrives (not the 20 s heartbeat).
    const pulled = uma.waitForResponse((r) => /\/scene\?since=/.test(r.url()) && r.status() === 200, { timeout: 8000 });
    await draw(vic, 1150, 300);
    const got = await (await pulled).json();
    if (got.unchanged || !got.scene) throw new Error('Uma was not sent the change');
    for (let i = 0; i < 20 && (await scene(boardId)).length <= before; i++) await vic.waitForTimeout(250);
    if ((await scene(boardId)).length !== before + 1) throw new Error('Vic\'s shape not saved');
  });

  await step('selected text becomes a task', async () => {
    await uma.locator('[data-excalidraw] canvas').last().click({ position: { x: 1000, y: 750 } });
    await uma.keyboard.press('Control+a');
    await uma.click('button:has-text("Make a task")');
    await uma.locator('text=/Task #\\d+ made/').waitFor();
    const tasks = (await api.get('/tasks?limit=50')).data.items;
    if (!tasks.some((t: any) => /Kanban|To do|Doing|Done/.test(t.title))) throw new Error(JSON.stringify(tasks.map((t: any) => t.title)));
  });

  let link = '';
  await step('a view link for someone outside', async () => {
    await uma.click('button:has-text("Share")');
    await uma.selectOption('select[aria-label="Link level"]', 'view');
    await uma.click('dialog button:has-text("Make a link")');
    link = (await uma.locator('[data-board-link]').textContent())!.trim();
    await uma.click('dialog button:has-text("Close")');
    const guest = await newPage();
    await guest.goto(link.replace(/^https?:\/\/[^/]+/, B));
    await guest.locator(`h1:has-text("Sprint wall ${s}")`).waitFor();
    await guest.locator('text=View only').waitFor();
    await guest.locator('[data-excalidraw] canvas').first().waitFor();
  });

  await step('a board inside a task', async () => {
    const t = (await api.post('/tasks', { title: `Sketch ${s}`, responsible_id: ids.Uma })).data;
    await uma.goto(`${B}/tasks/${t.id}`);
    uma.once('dialog', (d) => d.accept(`Task sketch ${s}`));
    await uma.click('[data-boards] button:has-text("New board")');
    await uma.waitForURL(/\/boards\/[0-9a-f-]+$/);
    await uma.locator(`a:has-text("Task #${t.number}")`).waitFor();
  });

  await step('no browser errors', async () => { if (errors.length) throw new Error(errors.join(' | ')); });
} finally {
  await browser.close();
  await cleanupRun(s);
}
console.log(failed ? `\n${failed} failed` : '\nall passed');
process.exit(failed ? 1 : 0);
