// Lists and processes in the browser: a list from a starting point, a process with an
// approval made in the designer, an item, the approver's inbox, the finished process.
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
const answers: string[] = [];
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `fail35-${n}-${i}.png`, fullPage: true }); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
await admin.post('/users', { email: `vic.${s}@acme.test`, password: 'password123', first_name: 'Vic', last_name: `Approver${s}` });
const login = async (email: string, password: string) => {
  const p = await (await browser.newContext({ viewport: { width: 1440, height: 1000 } })).newPage();
  p.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
  p.on('console', (m) => m.type() === 'error' && !/401|400|404|Failed to load resource/.test(m.text()) && errors.push(m.text()));
  p.on('dialog', (d) => d.accept(d.type() === 'prompt' ? answers.shift() : undefined));
  pages.push(p);
  await p.goto(B + '/login'); await p.fill('#email', email); await p.fill('#password', password);
  await p.click('button:has-text("Sign in")'); await p.waitForURL(B + '/');
  return p;
};
const ada = await login('admin@acme.test', 'supersecret1');
let listUrl = '';

try {
  await step('a list from a starting point', async () => {
    await ada.click('aside a:has-text("Lists")');
    await ada.click('button:has-text("New list")');
    await ada.fill('#ln-n', `Purchases ${s}`);
    await ada.selectOption('#ln-s', 'purchase');
    await ada.click('dialog[open] button:has-text("Create")');
    await ada.locator('[data-field="Amount"]').waitFor();
    listUrl = ada.url().split('?')[0];
  });

  await step('a process with an approval', async () => {
    answers.push(`Manager approval ${s}`);
    await ada.click('button:has-text("New process")');
    await ada.waitForURL(/\/lists\/workflow\//);
    await ada.selectOption('#wf-t', 'create');
    await ada.selectOption('select[aria-label="Step to add"]', 'approval');
    await ada.click('button:has-text("Add step")');
    const st = ada.locator('[data-step="approval"]');
    await st.locator('label:has-text("The author\'s head") input').uncheck();
    await st.locator('input[placeholder="And these people…"]').fill(`Approver${s}`);
    await st.locator(`button:has-text("Vic Approver${s}")`).click();
    await st.locator('[data-step="approval"] >> nth=0').count();
    // When approved: set Status to Approved.
    const approvedBranch = st.locator('div:has(> p:text-is("If approved"))');
    await approvedBranch.locator('select[aria-label="Step to add"]').selectOption('set_field');
    await approvedBranch.locator('button:has-text("Add step")').click();
    await approvedBranch.locator('select[aria-label="Field to set"]').selectOption({ label: 'Status' });
    await approvedBranch.locator('select[aria-label="New value"]').selectOption('Approved');
    await ada.click('button:has-text("Save")');
    await ada.locator('text=Saved').waitFor();
  });

  await step('an item starts it', async () => {
    await ada.goto(listUrl);
    await ada.click('button:has-text("New item")');
    await ada.fill('#it-title', `Chairs ${s}`);
    await ada.fill('#it-amount', '450');
    await ada.click('dialog[open] button:has-text("Save")');
    await ada.locator('[data-process="Manager approval ' + s + '"] .badge:text-is("Waiting")').waitFor();
  });

  await step('the approver approves from the inbox', async () => {
    const vic = await login(`vic.${s}@acme.test`, 'password123');
    await vic.locator('aside a:has-text("Approvals") >> text=1').waitFor();
    await vic.click('aside a:has-text("Approvals")');
    const card = vic.locator(`[data-approval="Approve Chairs ${s}"]`);
    await card.locator('input[aria-label="Comment"]').fill('Go ahead');
    await card.locator('button:has-text("Approve")').click();
    await card.waitFor({ state: 'detached' });
  });

  await step('approved', async () => {
    await ada.goto(listUrl);
    await ada.locator(`[data-item="Chairs ${s}"] [data-process-status="completed"]`).waitFor();
    await ada.click(`[data-item="Chairs ${s}"]`);
    await ada.locator('[data-item-view] >> text=Approved').waitFor();
    await ada.locator(`text=Vic Approver${s} approved: “Go ahead”`).waitFor();
  });
  await ada.screenshot({ path: '55-list-item.png' });
} finally {
  await cleanupRun(s);
}
console.log(errors.length ? 'console errors:\n' + errors.join('\n') : 'no console errors');
await browser.close();
process.exit(failed || errors.length ? 1 : 0);
