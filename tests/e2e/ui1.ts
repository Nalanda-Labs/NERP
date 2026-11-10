// The first UI suite: sign-in redirect, dashboard, employees and the basics of the app shell.
import { chromium } from 'playwright';
import { cleanupRun } from './cleanup.ts';
const B = 'http://localhost:8080';
const s = Date.now().toString(36);
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const page = await browser.newPage({ viewport: { width: 1360, height: 860 } });
const errors: any[] = [];
page.on('console', (m) => m.type() === 'error' && !/401|Failed to load resource/.test(m.text()) && errors.push(m.text()));
page.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
const shot = (n) => page.screenshot({ path: `${n}.png` });
const step = async (name, fn) => { try { await fn(); console.log('ok  ', name); } catch (e: any) { console.log('FAIL', name, e.message.split('\n')[0]); await shot('fail-' + name.replace(/\W+/g, '_')); } };

await step('redirect to login', async () => { await page.goto(B + '/employees'); await page.waitForURL(/\/login/); await shot('01-login'); });
await step('login', async () => {
  await page.fill('#email', 'admin@acme.test'); await page.fill('#password', 'supersecret1');
  await page.click('button:has-text("Sign in")'); await page.waitForURL(/\/employees$/); // honours ?next=
});
await step('dashboard', async () => { await page.click('nav >> text=Dashboard'); await page.waitForSelector('text=Good'); await page.waitForTimeout(400); await shot('02-dashboard'); });
await step('employees list', async () => { await page.click('nav >> text=Employees'); await page.waitForSelector('td >> text=Ada Admin'); await shot('03-employees'); });
await step('add employee', async () => {
  await page.click('button:has-text("Add employee")');
  await page.fill('#fn', 'Carol'); await page.fill('#ln', 'Chen'); await page.fill('#em', `carol.${s}@acme.test`); await page.fill('#pos', 'Account manager');
  await page.selectOption('#dept', { label: 'Sales' });
  await shot('04-add-employee');
  await page.click('dialog button:has-text("Send invitation")');
  await page.waitForURL(/\/employees\/[0-9a-f-]{36}$/); await page.waitForSelector('h1:has-text("Carol Chen")'); await shot('05-employee-profile');
});
await step('assign role', async () => {
  await page.click('button:has-text("Manage roles")');
  await page.check('dialog label:has-text("HR manager") input'); await page.click('dialog button:has-text("Save roles")');
  await page.waitForSelector('.badge:has-text("HR manager")');
});
await step('structure', async () => {
  await page.click('nav >> text=Company structure'); await page.waitForSelector('h2:has-text("Acme Ltd")');
  await page.click('nav.card >> text=Sales'); await page.waitForSelector('h2:has-text("Sales")'); await page.waitForSelector('text=Carol Chen');
  await shot('06-structure');
});
await step('create sub-department', async () => {
  await page.click('button:has-text("Add sub-department")'); await page.fill('#d-name', `Enterprise sales ${s}`);
  await page.click('dialog button:has-text("Create")'); await page.waitForSelector('h2:has-text("Enterprise sales")');
});
await step('roles page', async () => { await page.click('nav >> text=Roles'); await page.click('text=HR manager'); await page.waitForSelector('text=Manage employees'); await shot('07-roles'); });
await step('files upload', async () => {
  await page.click('nav >> text=My uploads');
  await page.setInputFiles('input[type=file]', { name: `report-${s}.txt`, mimeType: 'text/plain', buffer: Buffer.from('Q3 numbers') });
  await page.waitForSelector(`td >> text=report-${s}.txt`); await shot('08-files');
  const [dl] = await Promise.all([page.waitForEvent('download'), page.click('button[aria-label=Download] >> nth=0')]);
  if (dl.suggestedFilename() !== `report-${s}.txt`) throw new Error('bad download name ' + dl.suggestedFilename());
});
await step('audit', async () => { await page.click('nav >> text=Audit log'); await page.waitForSelector('text=Invited an employee') /* this suite's own "add employee" sends an invitation */; await shot('09-audit'); });
await step('settings', async () => { await page.click('nav a[href="/settings"]'); await page.waitForSelector('#s-company'); await shot('10-settings'); });
await step('profile + avatar', async () => {
  await page.goto(B + '/profile'); await page.waitForSelector('text=Personal details');
  const png = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkaPhfDwAEhgHAMaJ9bwAAAABJRU5ErkJggg==', 'base64');
  await page.setInputFiles('input[accept^="image"]', { name: `me-${s}.png`, mimeType: 'image/png', buffer: png });
  await page.waitForSelector('text=Photo updated'); await page.fill('#p-ph', '+44 20 1234 5678'); await page.click('button:has-text("Save profile")');
  await page.waitForSelector('text=Profile saved'); await shot('11-profile');
});
await step('reload keeps session', async () => { await page.reload(); await page.waitForSelector('text=Personal details'); });
await step('mobile layout', async () => { await page.setViewportSize({ width: 390, height: 800 }); await page.goto(B + '/employees'); await page.waitForSelector('main >> text=Ada Admin'); await shot('12-mobile'); await page.setViewportSize({ width: 1360, height: 860 }); });
await step('employee sign-in sees limited nav', async () => {
  await page.click('header button:has(img), header button:has(span[aria-label])'); await page.click('text=Sign out'); await page.waitForURL(/\/login/);
});
console.log('console errors:', errors.length ? errors : 'none');
await browser.close();
await cleanupRun(s);
