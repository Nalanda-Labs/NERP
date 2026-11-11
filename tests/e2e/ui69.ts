// Step 1 small items in the browser: the guided tour (offered once, skippable, played again
// from the menu), a request started from the feed and approved from the post, a CRM record's
// chat, and the export menu (Excel, CSV, vCard).
import { chromium } from 'playwright';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
const B = 'http://localhost:8080';
const s = Date.now().toString(36);
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: string[] = [];
let failed = 0, n = 0;
const pages: import('playwright').Page[] = [];
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `fail69-${n}-${i}.png`, fullPage: true }).catch(() => {}); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const mk = async (first: string) => {
  const email = `${first.toLowerCase()}.${s}@acme.test`;
  const u = (await admin.post('/users', { email, password: 'password123', first_name: first, last_name: `Ui${s}` })).data;
  return { id: u.id as string, email };
};
const bob = await mk('Bob'), cat = await mk('Cat');
const list = (await admin.post('/lists', { name: `Leave ${s}`, to_all: true, fields: [{ label: 'Days', kind: 'number', required: true }] })).data;
await admin.post(`/lists/${list.id}/workflows`, { name: `Approve ${s}`, trigger: 'create', steps: [{ kind: 'approval', title: 'Approve {{title}}', who: { users: [cat.id] }, mode: 'any', approved: [], rejected: [] }] });
const deal = (await admin.post('/crm/deal/items', { title: `Chat deal ${s}`, assigned_to: bob.id })).data;

const signIn = async (email: string, tours = false) => {
  const ctx = await browser.newContext({ viewport: { width: 1400, height: 950 }, timezoneId: 'UTC', locale: 'en-GB' });
  if (tours) await ctx.addInitScript(() => localStorage.setItem('nerp:tours-in-automation', '1'));
  const p = await ctx.newPage();
  p.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
  p.on('console', (m) => m.type() === 'error' && !/400|401|403|404|409|Failed to load resource/.test(m.text()) && errors.push(m.text()));
  pages.push(p);
  await p.goto(B + '/login'); await p.fill('#email', email); await p.fill('#password', 'password123');
  await p.click('button:has-text("Sign in")'); await p.waitForURL(B + '/');
  return p;
};

try {
  const pb = await signIn(bob.email, true);
  await step('a first visit offers the tour', async () => {
    await pb.locator('[data-tour-card]:has-text("News feed")').waitFor({ timeout: 10000 });
    await pb.click('[data-tour-card] button:has-text("Next")');
    await pb.locator('[data-tour-card]:has-text("Tasks")').waitFor();
    await pb.keyboard.press('ArrowLeft');
    await pb.locator('[data-tour-card]:has-text("News feed")').waitFor();
  });
  await step('skipping it is remembered', async () => {
    await pb.click('[data-tour-card] button:has-text("Skip the tour")');
    await pb.locator('[data-tour-card]').waitFor({ state: 'detached' });
    await pb.reload();
    await pb.waitForTimeout(2000);
    if (await pb.locator('[data-tour-card]').count()) throw new Error('offered again');
  });
  await step('"Take the tour" plays it again', async () => {
    await pb.click('[data-tour="me"]');
    await pb.click('[data-replay-tour]');
    await pb.locator('[data-tour-card]:has-text("News feed")').waitFor();
    await pb.keyboard.press('Escape');
    await pb.locator('[data-tour-card]').waitFor({ state: 'detached' });
  });

  await step('a request from the feed', async () => {
    await pb.goto(B + '/feed');
    await pb.click('[data-new-request]');
    await pb.selectOption('#rq-list', list.id);
    await pb.fill('#rq-title', `Holiday ${s}`);
    await pb.locator('dialog[open] input[type="number"]').fill('3');
    await pb.fill('#rq-note', `Off to the sea ${s}`);
    await pb.click('dialog[open] button:has-text("Send")');
    await pb.locator(`[data-request]:has-text("Holiday ${s}")`).waitFor();
  });
  const pc = await signIn(cat.email);
  await step('the approver approves it from the post', async () => {
    await pc.goto(B + '/feed');
    const post = pc.locator('article, [data-post]', { hasText: `Off to the sea ${s}` }).first();
    await post.locator('[data-my-approval] button:has-text("Approve")').click();
    await post.locator('[data-request-status="completed"]').waitFor();
  });

  await step('a CRM record\'s chat', async () => {
    await pb.goto(B + `/crm/deal/${deal.id}`);
    await pb.locator('h1', { hasText: `Chat deal ${s}` }).waitFor();
    await pb.locator('button:has-text("Chat")').first().click();
    await pb.waitForURL(/\/messenger\?chat=/);
    await pb.locator(`text=Deal #`).first().waitFor();
  });

  await step('the export menu: Excel, CSV and vCards for contacts', async () => {
    await pb.goto(B + '/crm/contact');
    await pb.click('[data-tour="crm-export"]');
    const items = await pb.locator('[data-export-menu] [role="menuitem"]').allInnerTexts();
    if (!items.some((t) => t.includes('Excel')) || !items.some((t) => t.includes('vCard'))) throw new Error(items.join(' | '));
    const [dl] = await Promise.all([pb.waitForEvent('download'), pb.click('[data-export-menu] button:has-text("Excel")')]);
    if (!dl.suggestedFilename().endsWith('.xlsx')) throw new Error(dl.suggestedFilename());
  });

  await step('no browser errors', async () => { if (errors.length) throw new Error(errors.join(' | ')); });
} finally {
  await browser.close();
  await cleanupRun(s);
}
console.log(failed ? `\n${failed} failed` : '\nall passed');
process.exit(failed ? 1 : 0);
