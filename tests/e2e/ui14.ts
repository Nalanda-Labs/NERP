// CRM analytics dashboard.
import { chromium } from 'playwright';
import { cleanupRun } from './cleanup.ts';
const B = 'http://localhost:8080';
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: any[] = [];
const ctx = await browser.newContext({ viewport: { width: 1440, height: 1000 } });
const page = await ctx.newPage();
page.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
page.on('console', (m) => m.type() === 'error' && !/401|400|Failed to load resource/.test(m.text()) && errors.push(m.text()));
let n = 0, failed = 0;
const step = async (name, fn) => { try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); await page.screenshot({ path: `fail14-${++n}.png`, fullPage: true }); } };
const s = Date.now().toString(36);
const api = async (method: string, path: string, body?: any, token?: string | null) => (await fetch(B + '/api' + path, { method, headers: { 'content-type': 'application/json', ...(token ? { authorization: `Bearer ${token}` } : {}) }, body: body ? JSON.stringify(body) : undefined })).json();
const token = (await api('POST', '/auth/login', { email: 'admin@acme.test', password: 'supersecret1' })).access_token;
const me = (await api('GET', '/auth/me', null, token));
const cfg = await api('GET', '/crm/config', null, token);
const cat = (await api('POST', '/crm/deal/categories', { name: `Dash ${s}` }, token)).categories.find((c) => c.name === `Dash ${s}`);
const open = cat.stages.filter((x) => x.semantics === 'process'), won = cat.stages.find((x) => x.semantics === 'success'), lost = cat.stages.find((x) => x.semantics === 'failure');
const ids: any[] = [];
for (const [amt, path, src] of [[1200, [open[1], open[2], won], 'website'], [800, [open[1], won], 'call'], [450, [open[1], lost], 'website'], [300, [], 'email'], [150, [open[1]], 'web_form']] as [number, any[], string][]) {
  const d = await api('POST', '/crm/deal/items', { title: `Dash ${s} ${amt}`, category_id: cat.id, currency: cfg.base_currency, opportunity: String(amt), source: src }, token);
  for (const st of path) await api('PATCH', `/crm/deal/items/${d.id}`, { stage_id: st.id }, token);
  ids.push(d.id);
}

await page.goto(B + '/login'); await page.fill('#email', 'admin@acme.test'); await page.fill('#password', 'supersecret1');
await page.click('button:has-text("Sign in")'); await page.waitForURL(B + '/');

await step('open from the sidebar', async () => {
  await page.click('aside >> a:has-text("Analytics")');
  await page.waitForURL(/\/crm\/analytics/);
  await page.locator('h1:text-is("Sales analytics")').waitFor();
  await page.locator('[data-tile="won"]').waitFor();
});

await step('pick the pipeline: figures and funnel', async () => {
  await page.selectOption('#an-pipe', cat.id);
  await page.waitForURL(new RegExp(`pipeline=${cat.id}`));
  await page.locator('[data-tile="won"] >> text=2 deals').waitFor();
  await page.locator('[data-tile="rate"] >> text=67%').waitFor();
  await page.locator('[data-tile="new"] p.text-2xl:text-is("5")').waitFor();
  await page.locator(`#fun-h >> xpath=../.. >> li:has-text("${open[0].name}") >> text=5`).waitFor();
  await page.locator(`#fun-h >> xpath=../.. >> li:has-text("${open[1].name}") >> text=4`).waitFor();
  await page.locator(`#fun-h >> xpath=../.. >> text=80% of previous`).waitFor();
});

await step('sales over time: hover, switch measure, table', async () => {
  const rects = page.locator('section:has(#dyn-h) svg rect');
  const count = await rects.count();
  if (count < 1) throw new Error('no bars');
  await rects.nth(count - 1).hover({ force: true });
  await page.locator('[role=tooltip] >> text=Won amount').waitFor();
  await page.click('button[role=tab]:has-text("New")');
  await page.locator('button[role=tab][aria-selected=true]:has-text("New")').waitFor();
  await page.click('button:has-text("Table")');
  await page.locator('section:has(#dyn-h) table >> text=Won amount').waitFor();
  await page.click('button:has-text("Chart")');
});

await step('managers and sources', async () => {
  await page.locator(`section:has(#mgr-h) tr:has-text("${me.first_name}")`).first().waitFor();
  await page.locator('section:has(#src-h) >> text=Website').waitFor();
  await page.locator('section:has(#src-h) >> text=Web form').waitFor();
  await page.screenshot({ path: '28-analytics.png', fullPage: true });
});

await step('set targets', async () => {
  await page.click('button:has-text("Set targets")');
  await page.fill('#t-team', '10000');
  await page.fill(`#t-${me.id}`, '4000');
  await page.click('button:has-text("Save targets")');
  await page.waitForSelector('text=Targets saved');
  await page.locator('[data-team-target] >> text=20%').waitFor();
  await page.locator(`section:has(#mgr-h) tr:has-text("${me.first_name}") [role=meter]`).waitFor();
});

await step('period presets and bookmarkable address', async () => {
  await page.selectOption('#an-period', 'last_year');
  await page.waitForURL(/period=last_year/);
  await page.locator('[data-tile="new"] p.text-2xl:text-is("0")').waitFor();
  await page.reload();
  if ((await page.inputValue('#an-period')) !== 'last_year' || (await page.inputValue('#an-pipe')) !== cat.id) throw new Error('filters not restored');
  await page.selectOption('#an-period', 'custom');
  await page.locator('#an-from').waitFor();
});

await page.goto(B + '/');
await api('PUT', '/crm/analytics/deal/targets', { month: new Date().toISOString().slice(0, 7), team_target: null, users: [] }, token);
for (const id of ids) { await api('DELETE', `/crm/deal/items/${id}`, null, token); await api('DELETE', `/crm/recycle-bin/${id}`, null, token); }
const gone = await fetch(B + `/api/crm/categories/${cat.id}`, { method: 'DELETE', headers: { authorization: `Bearer ${token}` } });
if (gone.status !== 200) errors.push('cleanup: pipeline not removed ' + gone.status);
console.log(errors.length ? 'console errors:\n' + errors.join('\n') : 'no console errors');
await browser.close();
await cleanupRun(s);
process.exit(failed || errors.length ? 1 : 0);
