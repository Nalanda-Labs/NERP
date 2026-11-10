// Workgroups in the browser: a closed workgroup with a post, files and an event; someone
// asks to join and is let in; the group chat.
import { chromium } from 'playwright';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
const B = 'http://localhost:8080';
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: string[] = [];
const watch = (p: import('playwright').Page) => {
  p.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
  p.on('console', (m) => m.type() === 'error' && !/401|400|404|Failed to load resource/.test(m.text()) && errors.push(m.text()));
  return p;
};
const s = Date.now().toString(36);
const answers: string[] = [];
const newPage = async () => {
  const p = watch(await (await browser.newContext({ viewport: { width: 1440, height: 1000 } })).newPage());
  p.on('dialog', (d) => d.accept(d.type() === 'prompt' ? answers.shift() : undefined));
  return p;
};
const page = await newPage();
let shot = page;
let n = 0, failed = 0;
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); await shot.screenshot({ path: `fail38-${++n}.png`, fullPage: true }); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
await admin.post('/users', { email: `cara.${s}@acme.test`, password: 'password123', first_name: 'Cara', last_name: `Joiner${s}` });
const login = async (p: import('playwright').Page, email: string, password: string) => {
  await p.goto(B + '/login'); await p.fill('#email', email); await p.fill('#password', password);
  await p.click('button:has-text("Sign in")'); await p.waitForURL(B + '/');
};
await login(page, 'admin@acme.test', 'supersecret1');
const name = `Guild ${s}`;
let url = '';

try {
  await step('a closed workgroup', async () => {
    await page.click('aside a:has-text("Workgroups")');
    await page.click('button:has-text("New workgroup")');
    await page.fill('#p-name', name);
    await page.fill('#p-desc', 'Designers talk shop');
    await page.check('dialog input[value="closed"]');
    await page.click('dialog button:has-text("Create workgroup")');
    await page.locator(`h1:has-text("${name}")`).waitFor();
    await page.locator('[data-group-meta] >> text=Workgroup · Closed').waitFor();
    await page.locator('[role="tab"][aria-selected="true"]:has-text("Feed")').waitFor();
    url = page.url();
  });

  await step('a post in the group feed', async () => {
    await page.fill('form[aria-label="New group post"] textarea', `Kick-off ${s}`);
    await page.click('form[aria-label="New group post"] button:has-text("Post")');
    await page.locator(`article >> text=Kick-off ${s}`).first().waitFor();
  });

  await step('group files', async () => {
    await page.click('[role="tab"]:has-text("Files")');
    answers.push(`Briefs ${s}`);
    await page.click('[aria-label="Group files"] button:has-text("New folder")');
    await page.click(`[data-group-node="Briefs ${s}"] button`);
    await page.setInputFiles('input[aria-label="Upload files to the group"]', { name: `brief-${s}.txt`, mimeType: 'text/plain', buffer: Buffer.from('hello') });
    await page.locator(`[data-group-node="brief-${s}.txt"]`).waitFor();
    await page.locator('nav[aria-label="Group folders"] >> text=Briefs').waitFor();
  });

  await step('a group event', async () => {
    await page.click('[role="tab"]:has-text("Calendar")');
    await page.click('button:has-text("New event")');
    await page.fill('#ev-title', `Critique ${s}`);
    await page.click('dialog button:has-text("Save")');
    await page.locator(`[data-group-event="Critique ${s}"]`).waitFor();
  });
  await page.screenshot({ path: '59-workgroup.png' });

  const cara = await newPage();
  shot = cara;
  await step('someone outside asks to join', async () => {
    await login(cara, `cara.${s}@acme.test`, 'password123');
    await cara.goto(url);
    await cara.locator('[data-group-card]').waitFor();
    answers.push('Can I join?');
    await cara.click('button:has-text("Ask to join")');
    await cara.locator('.badge:has-text("Request sent")').waitFor();
  });

  shot = page;
  await step('the owner lets them in', async () => {
    await page.reload();
    await page.click('[role="tab"]:has-text("Members")');
    await page.locator(`[data-request="Cara Joiner${s}"] >> text=“Can I join?”`).waitFor();
    await page.click(`[data-request="Cara Joiner${s}"] button:has-text("Let in")`);
    await page.locator(`a[href^="/employees/"] >> text=Cara Joiner${s}`).waitFor();
  });

  shot = cara;
  await step('now they see the group', async () => {
    await cara.reload();
    await cara.locator(`article >> text=Kick-off ${s}`).first().waitFor();
    await cara.click('[role="tab"]:has-text("Files")');
    await cara.locator(`[data-group-node="Briefs ${s}"]`).waitFor();
  });

  await step('the group chat', async () => {
    await cara.click('button:has-text("Chat")');
    await cara.waitForURL(/\/messenger\?chat=/);
    await cara.locator(`text=${name}`).first().waitFor();
  });
  shot = page;
} finally {
  await cleanupRun(s);
}
console.log(errors.length ? 'console errors:\n' + errors.join('\n') : 'no console errors');
await browser.close();
process.exit(failed || errors.length ? 1 : 0);
